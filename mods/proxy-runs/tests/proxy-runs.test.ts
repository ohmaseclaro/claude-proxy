import { expect, mock, test } from 'claude-code/testing'
import type { On, ProcessRunResult } from 'claude-code'

const TOOL = 'mcp__proxy-runs__run'
const SESSION = 'sess-1'

const RUN = {
  id: 'abc123',
  name: 'say ok',
  state: 'working',
  account: 'claude-gmail',
  turns: 1,
  activity: 'Bash ls',
  cost_usd: 0.12,
  asks: [],
  last_result: null,
  session: SESSION,
}
const TRANSCRIPT = Array.from({ length: 10 }, (_, i) => `  line ${i + 1}`).join('\n')

const ok = (stdout: string): { value: ProcessRunResult } => ({
  value: { exitCode: 0, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false },
})

type World = {
  argvs: string[][]
  stdins: (string | undefined)[]
  status: object
  events: string[]
  tools: string[]
  statuses: (string | undefined)[]
  toasts: string[]
  logs: string[]
  noProcess: boolean
  files: Record<string, string>
}

const META = {
  id: 'abc123',
  name: 'say ok',
  account: 'claude-gmail',
  state: 'working',
  turns: 1,
  activity: 'Bash ls',
  cost_usd: 0.12,
  last_result: null,
  session: SESSION,
}
const EVENTS_JSONL = [
  { event: 'turn_start', type: 'claude_proxy', account: 'claude-gmail', turn: 1 },
  { type: 'assistant', message: { content: [{ type: 'tool_use', name: 'Bash', input: { command: 'ls' } }] } },
  { type: 'assistant', message: { content: [{ type: 'text', text: 'ok' }] } },
]
  .map(e => JSON.stringify(e))
  .join('\n')

function world(on: On, events: string[] = [], env: Record<string, string> = {}): World {
  const w: World = {
    argvs: [],
    stdins: [],
    status: RUN,
    events,
    tools: [],
    statuses: [],
    toasts: [],
    logs: [],
    noProcess: false,
    files: {},
  }
  on('fs.list', () => ({ value: [{ name: 'abc123', kind: 'dir', size: 0, mtimeMs: 0, isLink: false }] }) as never)
  on('fs.read', ($, e) => ({ value: String(e.path).endsWith('meta.json') ? JSON.stringify(META) : EVENTS_JSONL }) as never)
  mock.env(on, { HOME: '/home/u', ...env })
  on('fs.write', ($, e) => ((w.files[e.path] = e.text), { value: undefined }))
  on('fs.exists', ($, e) => ({ value: e.path in w.files }))
  on('session.id', () => ({ value: SESSION }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('tool.register', ($, e) => {
    w.tools.push(`mcp__proxy-runs__${e.name}`)
    return { value: { tool: `mcp__proxy-runs__${e.name}` } } as never
  })
  on('command.register', () => ({ value: {} }) as never)
  on('ui.status', ($, e) => {
    w.statuses.push(e.text)
    return { value: undefined }
  })
  on('ui.toast', ($, e) => (w.toasts.push(e.text), { value: undefined }))
  on('ui.log', ($, e) => (w.logs.push(e.text), { value: undefined }))
  on('ui.open', () => ({ value: { isPlaced: true } }) as never)
  on('process.run', ($, e) => {
    if (w.noProcess) return { deny: 'CLI only' }
    w.argvs.push([...e.argv])
    w.stdins.push(e.init?.stdin)
    if (e.argv[0] === 'rm') {
      delete w.files[e.argv[2] ?? '']
      return ok('')
    }
    const [, cmd] = e.argv
    if (cmd === 'run' || cmd === 'status') return ok(JSON.stringify(w.status))
    if (cmd === 'runs') return ok(JSON.stringify([]))
    if (cmd === 'read') return ok(TRANSCRIPT)
    return ok(`${cmd} done`)
  })
  on('process.spawn', async function* () {
    for (const line of w.events) yield { stream: 'stdout' as const, text: `${line}\n` }
    return { value: { code: 0, signal: null } }
  })
  return w
}

const start = ($: { session: { start: (e: { cwd: string; surface: null; isInteractive: boolean }) => Promise<unknown> } }) =>
  $.session.start({ cwd: '/tmp', surface: null, isInteractive: false })

test('the tool starts a run and returns its id at once', async ($, on) => {
  const w = world(on)
  await start($)
  const ran = await $.tool.call({
    tool: TOOL,
    tool_use_id: 'tu1',
    prompt: 'reply with ok',
    description: 'say ok',
    subagent_type: 'gsd-executor',
    isolation: 'worktree',
    model: 'haiku',
  })
  expect(ran.deny).toBeUndefined()
  expect(String(ran.result)).toContain('abc123')
  expect(String(ran.result)).toContain('claude-proxy watch abc123')
  const i = w.argvs.findIndex(a => a[1] === 'run')
  expect(w.argvs[i]).toEqual([
    'claude-proxy', 'run', '-', '--json', '--name', 'say ok',
    '--agent', 'gsd-executor', '--worktree', '--', '--model', 'haiku',
  ])
  expect(w.stdins[i]).toBe('reply with ok')
})

test('built-in agent types are not passed as --agent', async ($, on) => {
  const w = world(on)
  await start($)
  await $.tool.call({ tool: TOOL, tool_use_id: 'tu2', prompt: 'p', description: 'look', subagent_type: 'Explore' })
  expect(w.argvs.find(a => a[1] === 'run')).toEqual(['claude-proxy', 'run', '-', '--json', '--name', 'look'])
})

test('the tool row draws the run collapsed and expanded on every surface', async ($, on) => {
  world(on, ['abc123 say ok · working · turn 1 on claude-gmail — Bash ls'])
  await start($)
  await $.tool.call({ tool: TOOL, tool_use_id: 'tu1', prompt: 'p', description: 'say ok' })
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({
      plugin: 'proxy-runs',
      surface,
      component: 'ToolUse',
      requestId: 'tu1',
      props: { tool_use_id: 'tu1', tool: TOOL, input: {}, isRunning: false, isErrored: false, isInterrupted: false },
    })
    expect((await ui.find({ type: 'Text', text: /run abc123/ }))?.text).toContain('say ok')
    expect(await ui.find({ type: 'Text', text: /claude-gmail/ })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: /line 10$/ })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: /line 4$/ })).toBeUndefined()
    await ui.press({ key: 'more:abc123' })
    expect(await ui.find({ type: 'Text', text: /line 1$/ })).toBeDefined()
    await ui.press({ key: 'more:abc123' })
    expect(await ui.find({ type: 'Text', text: /line 1$/ })).toBeUndefined()
    await ui.unmount()
  }
})

test('the Runs pane lists runs and its buttons call the right command', async ($, on) => {
  const w = world(on)
  w.status = { ...RUN, state: 'waiting', asks: [{ tool: 'Bash', input: { command: 'rm x' } }] }
  await start($)
  await $.tool.call({ tool: TOOL, tool_use_id: 'tu1', prompt: 'p', description: 'say ok' })
  const opened = await $.command.run({ command: 'runs', args: '', origin: { kind: 'user' } } as never)
  expect(opened).toMatchObject({ text: 'Runs pane opened.' })
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({
      plugin: 'proxy-runs',
      surface,
      component: 'Pane',
      requestId: 'runs',
      props: { title: 'Runs', isFocused: true, bodyColumns: 80, placement: 'dock' } as never,
    })
    expect((await ui.find({ key: 'pick:abc123' }))?.text).toContain('waiting')
    w.argvs = []
    await ui.press({ key: 'allow' })
    expect(w.argvs[0]).toEqual(['claude-proxy', 'allow', 'abc123'])
    w.argvs = []
    await ui.press({ key: 'kill' })
    expect(w.argvs[0]).toEqual(['claude-proxy', 'kill', 'abc123'])
    w.argvs = []
    await ui.input({ key: 'answer', text: 'Blue | Small' })
    expect(w.argvs[0]).toEqual(['claude-proxy', 'answer', 'abc123', 'Blue', 'Small'])
    await ui.unmount()
  }
})

test('a finished run wakes the session with its result', async ($, on) => {
  const w = world(on)
  let woke = (_: string) => {}
  const submitted = new Promise<string>(resolve => (woke = resolve))
  on('prompt.submit', ($, e) => {
    woke(e.text)
    return { text: e.text } as never
  })
  await start($)
  await $.tool.call({ tool: TOOL, tool_use_id: 'tu1', prompt: 'p', description: 'say ok' })
  w.status = { ...RUN, state: 'idle', last_result: { text: 'ok', ok: true } }
  w.events.push('abc123 say ok · idle · turn 1 on claude-gmail — ok')
  await start($)
  const text = await submitted
  expect(text).toContain('abc123 (say ok) is idle')
  expect(text).toContain('<run-result>\nok\n</run-result>')
})

test('without process access the pane reads the run files and says why', async ($, on) => {
  const w = world(on)
  w.noProcess = true
  const clock = mock.clock(on)
  await start($)
  await clock.advance(3000)
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({
      plugin: 'proxy-runs',
      surface,
      component: 'Pane',
      requestId: 'runs',
      props: { title: 'Runs', isFocused: true, bodyColumns: 80, placement: 'dock' } as never,
    })
    expect((await ui.find({ key: 'pick:abc123' }))?.text).toContain('working')
    expect(await ui.find({ type: 'Text', text: /cannot start claude-proxy/ })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: /⚙ Bash/ })).toBeDefined()
    await ui.press({ key: 'kill' })
    expect(w.toasts.at(-1)).toContain('in a terminal: claude-proxy kill abc123')
    await ui.unmount()
  }
})

test('without process access the tool refuses with a clear reason', async ($, on) => {
  const w = world(on)
  w.noProcess = true
  mock.clock(on)
  await start($)
  const ran = await $.tool.call({ tool: TOOL, tool_use_id: 'tu1', prompt: 'p', description: 'say ok' })
  expect(String(ran.isError ? ran.text : ran.deny)).toContain('cannot be started from this session')
})

const COMPOSE = {
  model: 'claude-opus-5-5',
  promptModel: 'claude-opus-5-5',
  surfaces: [],
  tools: [],
  outputStyle: null,
  traits: [],
} as never
const MARKER = `/home/u/.config/claude-proxy/sessions/${SESSION}.mod`
const agent = (description: string) => ({
  tool: 'Agent' as const,
  tool_use_id: 'ta',
  prompt: 'look around',
  description,
  subagent_type: 'Explore',
})
const spawned = (on: On) => on('tool.call', () => ({ result: 'spawned' }) as never)

test('Agent calls are denied toward the tool, except [direct]', async ($, on) => {
  world(on)
  spawned(on)
  await start($)
  const denied = await $.tool.call(agent('look around'))
  expect(String(denied.isError ? denied.text : denied.deny)).toContain(`Call ${TOOL}`)
  expect((await $.tool.call(agent('[direct] look around'))).result).toBe('spawned')
})

test('inside a run the mod stays silent', async ($, on) => {
  const w = world(on, [], { CLAUDE_PROXY_RUN: 'parent1' })
  spawned(on)
  on('prompt.compose', () => ({ sections: [] }))
  await start($)
  expect((await $.tool.call(agent('look'))).result).toBe('spawned')
  expect(w.tools).not.toContain(TOOL)
  expect(w.files).toEqual({})
  const composed = await $.prompt.compose(COMPOSE)
  expect(composed.sections).toHaveLength(0)
})

test('CLAUDE_PROXY_POLICY=off lets Agent calls through', async ($, on) => {
  world(on, [], { CLAUDE_PROXY_POLICY: 'off' })
  spawned(on)
  expect((await $.tool.call(agent('look'))).result).toBe('spawned')
})

test('the policy section tells the model to use the tool', async ($, on) => {
  world(on)
  on('prompt.compose', () => ({ sections: [{ id: 'intro', text: 'hi', scope: 'shared' }] }))
  const { sections } = await $.prompt.compose(COMPOSE)
  expect(sections.at(-1)).toMatchObject({ id: 'proxy-runs:policy', scope: 'session' })
  expect(sections.at(-1)?.text).toContain(`${TOOL} instead of \`claude-proxy run\` through Bash`)
})

test('the session marker is written at start and removed at end', async ($, on) => {
  const w = world(on)
  on('session.end', ($, e) => ({ sessionId: e.sessionId }) as never)
  await start($)
  expect(w.files[MARKER]).toBe('on')
  await $.session.end({ reason: 'other', sessionId: SESSION } as never)
  expect(MARKER in w.files).toBe(false)
})

test('without process access the marker is turned off instead', async ($, on) => {
  const w = world(on)
  mock.clock(on)
  on('session.end', ($, e) => ({ sessionId: e.sessionId }) as never)
  await start($)
  w.noProcess = true
  await $.session.end({ reason: 'other', sessionId: SESSION } as never)
  expect(w.files[MARKER]).toBe('off')
})
