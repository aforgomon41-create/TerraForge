import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseSeed } from '../src/generation/seed.ts'
test('blank seed requests automatic randomness, while zero is preserved', () => {
  assert.equal(parseSeed(''), undefined)
  assert.equal(parseSeed('  '), undefined)
  assert.equal(parseSeed('0'), 0)
  assert.equal(parseSeed('42'), 42)
  assert.equal(parseSeed('9007199254740991'), Number.MAX_SAFE_INTEGER)
})
test('unsafe or ambiguous seed input cannot silently create another seed', () => {
  for (const value of ['-1', '1.5', '1e3', '0x42', 'NaN', '9007199254740992'])
    assert.throws(() => parseSeed(value))
})
