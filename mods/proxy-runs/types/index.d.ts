export type ProxyRunsRun = {
  id: string
  name: string | null
  state: string
  account: string
  turns: number
  activity: string | null
  cost_usd: number
  asks: { tool: string; input: unknown }[]
  last_result: { text: string; ok: boolean } | null
  session: string | null
}

declare module 'claude-code' {
  interface PluginState {
    'proxy-runs': {
      runs: ProxyRunsRun[]
      calls: Record<string, string>
      told: Record<string, string>
      selected: string
      problem: string
      lines: StateFamily<string[]>
      expanded: StateFamily<boolean>
    }
  }
}
