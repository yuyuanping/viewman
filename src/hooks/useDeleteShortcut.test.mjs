import test from 'node:test';
import assert from 'node:assert/strict';
import { deletesOnKey } from './useDeleteShortcut.ts';

test('deletesOnKey fires only on Delete', () => {
  assert.equal(deletesOnKey('Delete', null), true);
  assert.equal(deletesOnKey('Backspace', null), false);
  assert.equal(deletesOnKey('d', null), false);
});

test('deletesOnKey stays out of the way while editing text', () => {
  assert.equal(deletesOnKey('Delete', { tagName: 'INPUT' }), false);
  assert.equal(deletesOnKey('Delete', { tagName: 'TEXTAREA' }), false);
  assert.equal(deletesOnKey('Delete', { tagName: 'DIV', isContentEditable: true }), false);
});

test('deletesOnKey works from buttons and the grid body', () => {
  assert.equal(deletesOnKey('Delete', { tagName: 'BUTTON' }), true);
  assert.equal(deletesOnKey('Delete', { tagName: 'BODY' }), true);
  assert.equal(deletesOnKey('Delete', { tagName: 'SELECT' }), true);
});
