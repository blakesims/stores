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
    RenderError, RenderErrorReason, ScopedJson,
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
        let a = h
            .param(0)
            .and_then(|p| p.value().as_f64())
            .ok_or_else(|| RenderError::from(RenderErrorReason::ParamNotFoundForIndex("gt", 0)))?;
        let b = h
            .param(1)
            .and_then(|p| p.value().as_f64())
            .ok_or_else(|| RenderError::from(RenderErrorReason::ParamNotFoundForIndex("gt", 1)))?;
        Ok(ScopedJson::Derived(json!(a > b)))
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
        let a = h
            .param(0)
            .and_then(|p| p.value().as_f64())
            .ok_or_else(|| RenderError::from(RenderErrorReason::ParamNotFoundForIndex("lt", 0)))?;
        let b = h
            .param(1)
            .and_then(|p| p.value().as_f64())
            .ok_or_else(|| RenderError::from(RenderErrorReason::ParamNotFoundForIndex("lt", 1)))?;
        Ok(ScopedJson::Derived(json!(a < b)))
    }
}

/// `{{default value "fallback"}}` — emits fallback when value is missing / null / empty.
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
pub fn render_template(text: &str, ctx: &Value) -> Result<String> {
    let mut hbs = Handlebars::new();
    // Strict mode OFF — missing keys → empty string, never an error.
    hbs.set_strict_mode(false);

    hbs.register_helper("eq", Box::new(EqHelper));
    hbs.register_helper("gt", Box::new(GtHelper));
    hbs.register_helper("lt", Box::new(LtHelper));
    hbs.register_helper("default", Box::new(helper_default));

    let rendered = hbs
        .render_template(text, ctx)
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
}
