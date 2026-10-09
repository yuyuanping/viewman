import test from 'node:test';
import assert from 'node:assert/strict';
import { logger } from './logger.ts';

test('logger exposes four levels without throwing', () => {
  for (const level of ['debug', 'info', 'warn', 'error']) {
    assert.equal(typeof logger[level], 'function');
    logger[level]('test', 'hello', { n: 1 });
  }
});
