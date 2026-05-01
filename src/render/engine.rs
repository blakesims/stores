/// Template rendering engine — wraps `handlebars::Handlebars` with the custom
/// helpers needed by briefing and main.md templates.
///
/// Helpers registered:
///   `eq`      — `{{#if (eq a b)}}` equality check (subexpression-safe)
///   `default` — `{{default field "—"}}` fallback for missing/null/empty values
///   `gt`      — `{{#if (gt a b)}}` greater-than comparison (numbers)
///   `lt`      — `{{#if (lt a b)}}` less-than comparison (numbers)
use anyhow::Result;
use handlebars::{
    Context, Handlebars, Helper, HelperDef, HelperResult, JsonRender, Output, RenderContext,
    RenderError, ScopedJson,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Custom helpers — use `call_inner` so they compose correctly as subexpressions
// inside `{{#if (eq …)}}`.  Helpers that return `ScopedJson` are type-aware;
// `{{#if}}` uses the JSON boolean value, not a string parse.
// ---------------------------------------------------------------------------

struct EqHelper;
impl HelperDef for EqHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> std::result::Result<ScopedJson<'rc>, RenderError> {
        let a = h.param(0).map(|p| p.value().render()).unwrap_or_default();
        let b = h.param(1).map(|p| p.value().render()).unwrap_or_default();
        Ok(ScopedJson::Derived(json!(a == b)))
    }
}

struct GtHelper;
impl HelperDef for GtHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> std::result::Result<ScopedJson<'rc>, RenderError> {
        // Missing or non-numeric param → false (never error). Mirrors EqHelper
        // leniency: render must never crash on partial DB rows (plan task 3.4).
        let a = h.param(0).and_then(|p| p.value().as_f64());
        let b = h.param(1).and_then(|p| p.value().as_f64());
        match (a, b) {
            (Some(a), Some(b)) => Ok(ScopedJson::Derived(json!(a > b))),
            _ => Ok(ScopedJson::Derived(json!(false))),
        }
    }
}

struct LtHelper;
impl HelperDef for LtHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> std::result::Result<ScopedJson<'rc>, RenderError> {
        // Missing or non-numeric param → false (never error). Mirrors EqHelper
        // leniency: render must never crash on partial DB rows (plan task 3.4).
        let a = h.param(0).and_then(|p| p.value().as_f64());
        let b = h.param(1).and_then(|p| p.value().as_f64());
        match (a, b) {
            (Some(a), Some(b)) => Ok(ScopedJson::Derived(json!(a < b))),
            _ => Ok(ScopedJson::Derived(json!(false))),
        }
    }
}

struct AddHelper;
impl HelperDef for AddHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> std::result::Result<ScopedJson<'rc>, RenderError> {
        let a = h.param(0).and_then(|p| p.value().as_i64()).unwrap_or(0);
        let b = h.param(1).and_then(|p| p.value().as_i64()).unwrap_or(0);
        Ok(ScopedJson::Derived(json!(a + b)))
    }
}

/// `{{default value "fallback"}}` — emits fallback when value is missing / null / empty string.
/// Note: `0`, `false`, and empty arrays/objects are NOT treated as empty and pass through as-is.
/// Template authors that need those cases should guard with `{{#if}}` instead.
fn helper_default(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let value = h.param(0);
    let fallback = h
        .param(1)
        .map(|p| {
            let v = p.value();
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.render()
            }
        })
        .unwrap_or_default();

    let rendered = match value {
        None => fallback,
        Some(p) => {
            let v = p.value();
            if v.is_null() {
                fallback
            } else {
                let s = if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.render()
                };
                if s.is_empty() { fallback } else { s }
            }
        }
    };
    out.write(&rendered)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render `text` as a Handlebars template against `ctx`.
///
/// - Missing keys render as empty string (Handlebars default, strict mode off).
/// - Returns `Err` only on template syntax errors, not on missing data.
///
/// # Performance note (TODO Phase 6)
/// A fresh `Handlebars` registry and four helpers are constructed on every call.
/// If profiling shows this is a bottleneck for bulk renders, consider a
/// `OnceLock<Handlebars<'static>>` or a `RenderEngine` struct that wraps a
/// reusable registry (helpers hold no per-render state).
pub fn render_template(text: &str, ctx: &Value) -> Result<String> {
    render_template_with_overlay(text, ctx, &std::collections::HashMap::new())
}

/// Render `text` as a Handlebars template against `ctx`, with an additional
/// `overlay` map merged on top of `ctx` before template evaluation.
///
/// The overlay is a caller-supplied `HashMap<String, Value>` that is shallow-
/// merged into the top-level context object.  Keys in the overlay take
/// precedence over identically-named keys already present in `ctx`.
///
/// Non-wrap callers pass an empty overlay via `render_template` and observe no
/// behavioural change.  The wrap brief path in `drive.rs` passes
/// `git_diff_summary` so render stays pure `(schema, entry) → Value` — the
/// diff computation never enters `src/render/context.rs`.
///
/// # AC4.5 — render stays pure
/// `src/render/context.rs::build_context` is NOT modified.  The overlay is a
/// separate parameter that is merged at template-evaluation time only.
pub fn render_template_with_overlay(
    text: &str,
    ctx: &Value,
    overlay: &std::collections::HashMap<String, Value>,
) -> Result<String> {
    // Merge overlay into a shallow clone of ctx.
    let merged: Value = if overlay.is_empty() {
        ctx.clone()
    } else {
        let mut map = match ctx {
            Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        for (k, v) in overlay {
            map.insert(k.clone(), v.clone());
        }
        Value::Object(map)
    };

    let mut hbs = Handlebars::new();
    // Strict mode OFF — missing keys → empty string, never an error.
    hbs.set_strict_mode(false);

    hbs.register_helper("eq", Box::new(EqHelper));
    hbs.register_helper("gt", Box::new(GtHelper));
    hbs.register_helper("lt", Box::new(LtHelper));
    hbs.register_helper("add", Box::new(AddHelper));
    hbs.register_helper("default", Box::new(helper_default));

    let rendered = hbs
        .render_template(text, &merged)
        .map_err(|e| anyhow::anyhow!("template render error: {}", e))?;
    Ok(rendered)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // AC3.1: static template (no {{…}}) round-trips byte-identical.
    #[test]
    fn static_template_roundtrips() {
        let tpl = "Hello, world!\nNo substitutions here.";
        let ctx = json!({});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, tpl);
    }

    // AC3.2: {{title}} substitutes from context.
    #[test]
    fn variable_substitution() {
        let tpl = "Title: {{title}}";
        let ctx = json!({"title": "My Task"});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "Title: My Task");
    }

    // AC3.2: missing variable renders empty (not an error).
    #[test]
    fn missing_variable_renders_empty() {
        let tpl = "A: {{missing_key}}";
        let ctx = json!({});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "A: ");
    }

    // AC3.2: null value renders empty.
    #[test]
    fn null_variable_renders_empty() {
        let tpl = "A: {{val}}";
        let ctx = json!({"val": null});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "A: ");
    }

    // AC3.3: {{#each list}} iterates array.
    #[test]
    fn each_iterates_list() {
        let tpl = "{{#each phases}}- {{this.name}}\n{{/each}}";
        let ctx = json!({"phases": [{"name": "Alpha"}, {"name": "Beta"}]});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "- Alpha\n- Beta\n");
    }

    // AC3.4: {{#if (eq status "BLOCKED")}} conditional — true branch.
    #[test]
    fn if_eq_helper_true_branch() {
        let tpl = "{{#if (eq status \"BLOCKED\")}}blocked!{{/if}}";
        let ctx = json!({"status": "BLOCKED"});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "blocked!");
    }

    // AC3.4: {{#if (eq status "BLOCKED")}} conditional — false / else branch.
    #[test]
    fn if_eq_helper_false_branch() {
        let tpl = "{{#if (eq status \"BLOCKED\")}}blocked!{{else}}not blocked{{/if}}";
        let ctx = json!({"status": "executing"});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "not blocked");
    }

    // default helper — fallback for missing key.
    #[test]
    fn default_helper_uses_fallback_for_missing() {
        let tpl = "{{default missing_key \"—\"}}";
        let ctx = json!({});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "—");
    }

    // default helper — uses value when present.
    #[test]
    fn default_helper_uses_value_when_present() {
        let tpl = "{{default name \"—\"}}";
        let ctx = json!({"name": "Alice"});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "Alice");
    }

    // default helper — fallback for null.
    #[test]
    fn default_helper_null_uses_fallback() {
        let tpl = "{{default val \"N/A\"}}";
        let ctx = json!({"val": null});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "N/A");
    }

    // gt helper — true case.
    #[test]
    fn gt_helper_true() {
        let tpl = "{{#if (gt a b)}}yes{{/if}}";
        let ctx = json!({"a": 5, "b": 3});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "yes");
    }

    // gt helper — false case.
    #[test]
    fn gt_helper_false() {
        let tpl = "{{#if (gt a b)}}yes{{else}}no{{/if}}";
        let ctx = json!({"a": 1, "b": 3});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "no");
    }

    // lt helper — true case.
    #[test]
    fn lt_helper_true() {
        let tpl = "{{#if (lt a b)}}yes{{/if}}";
        let ctx = json!({"a": 1, "b": 3});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "yes");
    }

    // lt helper — false case.
    #[test]
    fn lt_helper_false() {
        let tpl = "{{#if (lt a b)}}yes{{else}}no{{/if}}";
        let ctx = json!({"a": 5, "b": 3});
        let out = render_template(tpl, &ctx).unwrap();
        assert_eq!(out, "no");
    }

    // Regression: gt missing first arg → false, no error (plan task 3.4 contract).
    #[test]
    fn gt_helper_missing_key_returns_false() {
        // Missing top-level key on first arg.
        let out = render_template(
            "{{#if (gt missing_key 3)}}yes{{else}}no{{/if}}",
            &json!({}),
        )
        .unwrap();
        assert_eq!(out, "no");

        // Missing key on second arg.
        let out = render_template(
            "{{#if (gt 5 missing_key)}}yes{{else}}no{{/if}}",
            &json!({}),
        )
        .unwrap();
        assert_eq!(out, "no");

        // Non-numeric value (string) → false.
        let out = render_template(
            "{{#if (gt val 3)}}yes{{else}}no{{/if}}",
            &json!({"val": "hello"}),
        )
        .unwrap();
        assert_eq!(out, "no");

        // Both keys missing → false.
        let out = render_template(
            "{{#if (gt a b)}}yes{{else}}no{{/if}}",
            &json!({}),
        )
        .unwrap();
        assert_eq!(out, "no");
    }

    // AC4.5: render_template_with_overlay_merges_correctly — overlay keys appear
    // in rendered output; context-only keys still work; overlay wins on conflict.
    #[test]
    fn render_template_with_overlay_merges_correctly() {
        // 1. Overlay key reaches template.
        let tpl = "diff: {{git_diff_summary}}";
        let ctx = json!({});
        let mut overlay = std::collections::HashMap::new();
        overlay.insert("git_diff_summary".to_string(), json!("abc123..HEAD: 3 files changed"));
        let out = super::render_template_with_overlay(tpl, &ctx, &overlay).unwrap();
        assert_eq!(out, "diff: abc123..HEAD: 3 files changed");

        // 2. Context-only keys still work when overlay is non-empty.
        let tpl2 = "title: {{title}} diff: {{git_diff_summary}}";
        let ctx2 = json!({"title": "My Task"});
        let out2 = super::render_template_with_overlay(tpl2, &ctx2, &overlay).unwrap();
        assert_eq!(out2, "title: My Task diff: abc123..HEAD: 3 files changed");

        // 3. Overlay wins over context on key conflict.
        let ctx3 = json!({"git_diff_summary": "old value"});
        let out3 = super::render_template_with_overlay(tpl, &ctx3, &overlay).unwrap();
        assert_eq!(out3, "diff: abc123..HEAD: 3 files changed");

        // 4. Empty overlay → same as render_template.
        let empty: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
        let ctx4 = json!({"val": "hello"});
        let tpl4 = "val: {{val}}";
        let out4 = super::render_template_with_overlay(tpl4, &ctx4, &empty).unwrap();
        assert_eq!(out4, "val: hello");
    }

    // Regression: lt missing first arg → false, no error (plan task 3.4 contract).
    #[test]
    fn lt_helper_missing_key_returns_false() {
        // Missing top-level key on first arg.
        let out = render_template(
            "{{#if (lt missing_key 3)}}yes{{else}}no{{/if}}",
            &json!({}),
        )
        .unwrap();
        assert_eq!(out, "no");

        // Missing key on second arg.
        let out = render_template(
            "{{#if (lt 1 missing_key)}}yes{{else}}no{{/if}}",
            &json!({}),
        )
        .unwrap();
        assert_eq!(out, "no");

        // Non-numeric value (string) → false.
        let out = render_template(
            "{{#if (lt val 3)}}yes{{else}}no{{/if}}",
            &json!({"val": "hello"}),
        )
        .unwrap();
        assert_eq!(out, "no");

        // Both keys missing → false.
        let out = render_template(
            "{{#if (lt a b)}}yes{{else}}no{{/if}}",
            &json!({}),
        )
        .unwrap();
        assert_eq!(out, "no");
    }
}
