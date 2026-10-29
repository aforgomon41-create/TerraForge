import { test } from 'node:test'
import assert from 'node:assert/strict'
import { TerraAgent, validateConfig } from '../src/agent/core.ts'
const config = {
  preset: 'custom',
  baseUrl: 'https://example.test/v1',
  model: 'user-selected-model',
  apiKey: 'test-only',
}
const prompt = 'Test'
const originalFetch = globalThis.fetch
function response(parts: unknown[]) {
  return new Response(
    parts
      .map((p) => 'data: ' + (typeof p === 'string' ? p : JSON.stringify(p)) + '\r\n\r\n')
      .join(''),
    { headers: { 'content-type': 'text/event-stream' } },
  )
}
const final = (text = 'Answer') =>
  response([{ choices: [{ delta: { content: text }, finish_reason: 'stop' }] }, '[DONE]'])
function toolCall(calls: unknown[], text?: string) {
  return response([
    {
      choices: [
        {
          delta: { content: text, tool_calls: calls },
          finish_reason: 'tool_calls',
        },
      ],
    },
    '[DONE]',
  ])
}
const wire = (index: number, id: string, name: string, args: string) => ({
  index,
  id,
  type: 'function',
  function: { name, arguments: args },
})
const tool = {
  name: 'inspect',
  description: 'Read test data',
  parameters: {
    type: 'object',
    properties: { value: { type: 'number' } },
    required: ['value'],
  },
  execute: async (args: { value: number }) => ({ value: args.value + 1 }),
}
test('pure tools preserve user, assistant calls, matching results and later turns', async () => {
  const bodies: any[] = []
  let count = 0
  globalThis.fetch = async (_url, init) => {
    bodies.push(JSON.parse(String(init?.body)))
    return count++ === 0 ? toolCall([wire(0, 'one', 'inspect', '{"value":2}')]) : final()
  }
  try {
    const agent = new TerraAgent(config, [tool], prompt)
    let errors: string[] = []
    await agent.send('Question', { onError: (e) => errors.push(e) })
    await agent.send('Follow-up', {})
    assert.deepEqual(errors, [])
    assert.deepEqual(
      bodies[1].messages.map((m: any) => m.role),
      ['system', 'user', 'assistant', 'tool'],
    )
    assert.equal(bodies[1].messages[1].content, 'Question')
    assert.equal(bodies[1].messages[2].tool_calls[0].id, 'one')
    assert.equal(bodies[1].messages[3].tool_call_id, 'one')
    assert.equal(JSON.parse(bodies[1].messages[3].content).value, 3)
    assert.equal(bodies[2].messages.at(-1).content, 'Follow-up')
  } finally {
    globalThis.fetch = originalFetch
  }
})
test('text with multiple calls and tool failure produces a result for every ID', async () => {
  let count = 0
  const bodies: any[] = []
  globalThis.fetch = async (_url, init) => {
    bodies.push(JSON.parse(String(init?.body)))
    return count++ === 0
      ? toolCall(
          [wire(0, 'a', 'inspect', '{"value":1}'), wire(1, 'b', 'unknown', '{}')],
          'Checking',
        )
      : final()
  }
  try {
    const agent = new TerraAgent(config, [tool], prompt)
    await agent.send('Question', {})
    assert.equal(bodies[1].messages[2].content, 'Checking')
    assert.deepEqual(
      bodies[1].messages.slice(3).map((m: any) => m.tool_call_id),
      ['a', 'b'],
    )
    assert.ok(JSON.parse(bodies[1].messages[4].content).error)
  } finally {
    globalThis.fetch = originalFetch
  }
})
test('invalid arguments do not execute a tool', async () => {
  let executed = false,
    count = 0
  globalThis.fetch = async () =>
    count++ === 0 ? toolCall([wire(0, 'a', 'inspect', '{"value":"bad"}')]) : final()
  try {
    const agent = new TerraAgent(
      config,
      [
        {
          ...tool,
          execute: async () => {
            executed = true
          },
        },
      ],
      prompt,
    )
    await agent.send('Question', {})
    assert.equal(executed, false)
    assert.match(agent.history().find((m) => m.role === 'tool')!.content!, /error/)
  } finally {
    globalThis.fetch = originalFetch
  }
})
test('aborting cancels an outstanding network request and retains a coherent history', async () => {
  let started: () => void
  const pending = new Promise<void>((r) => {
    started = r
  })
  globalThis.fetch = (_url, init) =>
    new Promise((_resolve, reject) => {
      init?.signal?.addEventListener(
        'abort',
        () => reject(new DOMException('Aborted', 'AbortError')),
        { once: true },
      )
      started()
    })
  try {
    const agent = new TerraAgent(config, [], prompt)
    let error = ''
    const sending = agent.send('Question', {
      onError: (e) => {
        error = e
      },
    })
    await pending
    agent.abort()
    await sending
    assert.match(error, /停止/)
    assert.equal(agent.history()[0].role, 'user')
    assert.equal(agent.history().at(-1)?.role, 'assistant')
  } finally {
    globalThis.fetch = originalFetch
  }
})
test('stream failure always reports and closes the turn', async () => {
  globalThis.fetch = async () => response([{ choices: [{ delta: { content: 'Partial' } }] }])
  try {
    const agent = new TerraAgent(config, [], prompt)
    let error = ''
    await agent.send('Question', {
      onError: (e) => {
        error = e
      },
    })
    assert.match(error, /中断/)
    assert.equal(agent.history()[0].role, 'user')
    assert.equal(agent.history().at(-1)?.role, 'assistant')
  } finally {
    globalThis.fetch = originalFetch
  }
})
test('final SSE line without newline is parsed', async () => {
  globalThis.fetch = async () =>
    new Response(
      'data: ' +
        JSON.stringify({
          choices: [{ delta: { content: 'End' }, finish_reason: 'stop' }],
        }),
    )
  try {
    const agent = new TerraAgent(config, [], prompt)
    let text = ''
    await agent.send('Question', { onText: (t) => (text += t) })
    assert.equal(text, 'End')
  } finally {
    globalThis.fetch = originalFetch
  }
})
test('provider config requires an explicit model and protects credentials in URLs', () => {
  assert.throws(() => validateConfig({ ...config, model: '' }))
  assert.throws(() => validateConfig({ ...config, baseUrl: 'http://public.test/v1' }))
  assert.throws(() => validateConfig({ ...config, baseUrl: 'https://user:pass@example.test' }))
  assert.doesNotThrow(() => validateConfig({ ...config, baseUrl: 'http://127.0.0.1:1234/v1' }))
})
