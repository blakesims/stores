#!/usr/bin/env node
import fs from 'node:fs';
import process from 'node:process';
async function importFirst(specs) {
  let last;
  for (const spec of specs) {
    try { return await import(spec); } catch (e) { last = e; }
  }
  throw last;
}

const piSdkPath = process.env.STORES_PI_SDK_PATH ?? '/home/blake/.npm-global/lib/node_modules/@mariozechner/pi-coding-agent/dist/index.js';
const typeboxPath = process.env.STORES_TYPEBOX_PATH ?? '/home/blake/.npm-global/lib/node_modules/@mariozechner/pi-coding-agent/node_modules/typebox/build/index.mjs';
const { Type } = await importFirst(['typebox', typeboxPath]);
const {
  AuthStorage,
  createAgentSession,
  DefaultResourceLoader,
  defineTool,
  ModelRegistry,
  SessionManager,
  SettingsManager,
} = await importFirst(['@mariozechner/pi-coding-agent', piSdkPath]);

function arg(name) {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const role = arg('role');
const cwd = arg('cwd');
const systemPath = arg('system');
const briefPath = arg('brief');
const schemaPath = arg('schema');
if (!role || !cwd || !systemPath || !briefPath) {
  console.error('usage: pi_runner.mjs --role ROLE --cwd CWD --system FILE --brief FILE [--schema FILE]');
  process.exit(2);
}

const systemPrompt = fs.readFileSync(systemPath, 'utf8');
const brief = fs.readFileSync(briefPath, 'utf8');
const roleSchema = schemaPath ? JSON.parse(fs.readFileSync(schemaPath, 'utf8')) : undefined;
let finalPayload;

const finalOutputTool = defineTool({
  name: 'final_output',
  label: 'Final Output',
  description: 'Terminate the stores runner and submit the final structured payload. The payload must satisfy the role JSON schema provided in the prompt.',
  // Pi tools use TypeBox schemas. Full JSON-Schema-to-TypeBox translation is deliberately
  // narrow for the first runner: accept any JSON payload at tool-call time, then the Rust
  // runner validates against the authoritative role schema before submitting.
  parameters: Type.Object({ payload: Type.Any() }),
  execute: async (_toolCallId, params) => {
    finalPayload = params.payload;
    const event = { type: 'final_output', role, payload: finalPayload };
    process.stdout.write(`${JSON.stringify(event)}\n`);
    return { content: [{ type: 'text', text: 'final_output received; stop now.' }], details: event };
  },
});

const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false } });
const agentDir = fs.mkdtempSync('/tmp/stores-pi-agent-');
const loader = new DefaultResourceLoader({
  cwd,
  agentDir,
  settingsManager,
  systemPromptOverride: () => systemPrompt,
  skillsOverride: () => ({ skills: [], diagnostics: [] }),
  promptsOverride: () => ({ prompts: [], diagnostics: [] }),
  agentsFilesOverride: () => ({ agentsFiles: [] }),
});
await loader.reload();

const authStorage = AuthStorage.create();
const modelRegistry = ModelRegistry.create(authStorage);
const { session } = await createAgentSession({
  cwd,
  agentDir,
  authStorage,
  modelRegistry,
  resourceLoader: loader,
  settingsManager,
  sessionManager: SessionManager.inMemory(),
  tools: ['read', 'bash', 'edit', 'write', 'final_output'],
  customTools: [finalOutputTool],
});

session.subscribe((event) => process.stdout.write(`${JSON.stringify(event)}\n`));

const schemaText = roleSchema ? `\n\nAuthoritative output JSON schema:\n\`\`\`json\n${JSON.stringify(roleSchema)}\n\`\`\`` : '';
const finalInstruction = `${schemaText}\n\nTERMINATION REQUIREMENT: before ending your response, you MUST call the final_output tool exactly once with the complete JSON payload for role ${role}. Do not print the payload as text instead of calling the tool. If you used other tools, continue until you can call final_output.`;

await session.prompt(`${brief}${finalInstruction}`);
await session.agent.waitForIdle();

for (let i = 0; finalPayload === undefined && i < 6; i++) {
  await session.prompt(`You have not called final_output yet. Call final_output now with the complete ${role} JSON payload. Do not use any other tool unless absolutely required.`);
  await session.agent.waitForIdle();
}

session.dispose();

if (finalPayload === undefined) {
  console.error('pi runner: missing final_output tool call');
  process.exit(3);
}
