import test from 'node:test'
import assert from 'node:assert/strict'
import { evolutionClock } from '../src/generation/progress.ts'

test('the long history clock explicitly switches to the recent detailed window', () => {
  assert.equal(evolutionClock(4000, 4600), '远古简化演化 4000.0/4480.0 百万年')
  assert.equal(evolutionClock(4480, 4600), '近期精细演化 0.0/120.0 百万年')
  assert.equal(evolutionClock(4595, 4600), '近期精细演化 115.0/120.0 百万年')
  assert.equal(evolutionClock(4600.000001, 4600), '近期精细演化 120.0/120.0 百万年')
})

test('short runs and missing legacy durations do not pretend to have an ancient window', () => {
  assert.equal(evolutionClock(5, 10), '近期精细演化 5.0/10.0 百万年')
  assert.equal(evolutionClock(3, NaN), '构造时间 3.0 百万年')
  assert.equal(evolutionClock(0, 0), '构造时间 0.0 百万年')
})
