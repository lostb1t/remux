import { test } from 'node:test';
import assert from 'node:assert/strict';
import { loadPatchedDom, nextTick } from './helpers.mjs';

test('strips "Recently Added in " from a homescreen section title', async () => {
  const dom = loadPatchedDom();
  const { document } = dom.window;

  const title = document.createElement('h2');
  title.className = 'sectionTitle';
  title.textContent = 'Recently Added in Movies';
  document.body.appendChild(title);

  await nextTick();

  assert.equal(title.textContent, 'Movies');
});

test('leaves a section title without the prefix untouched', async () => {
  const dom = loadPatchedDom();
  const { document } = dom.window;

  const title = document.createElement('h2');
  title.className = 'sectionTitleLink';
  title.textContent = 'Continue Watching';
  document.body.appendChild(title);

  await nextTick();

  assert.equal(title.textContent, 'Continue Watching');
});
