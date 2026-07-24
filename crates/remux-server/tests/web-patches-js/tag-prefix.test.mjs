import { test } from 'node:test';
import assert from 'node:assert/strict';
import { loadPatchedDom, nextTick } from './helpers.mjs';

test('strips "provider:" from a tag link label but leaves the href untouched', async () => {
  const dom = loadPatchedDom();
  const { document } = dom.window;

  const container = document.createElement('div');
  container.className = 'itemTags';
  const anchor = document.createElement('a');
  anchor.className = 'button-link';
  anchor.setAttribute('href', '/web/index.html#!/list.html?tag=provider%3AHBO%20Max');
  anchor.textContent = 'provider:HBO Max';
  container.appendChild(anchor);
  document.body.appendChild(container);

  await nextTick();

  assert.equal(anchor.textContent, 'HBO Max');
  assert.equal(
    anchor.getAttribute('href'),
    '/web/index.html#!/list.html?tag=provider%3AHBO%20Max',
    'href must keep the raw provider:-prefixed value so tag-click filtering still matches the DB',
  );
});

test('leaves a non-provider tag untouched', async () => {
  const dom = loadPatchedDom();
  const { document } = dom.window;

  const container = document.createElement('div');
  container.className = 'itemTags';
  const anchor = document.createElement('a');
  anchor.textContent = 'IMDb';
  container.appendChild(anchor);
  document.body.appendChild(container);

  await nextTick();

  assert.equal(anchor.textContent, 'IMDb');
});

test('only matches the literal "provider:" prefix, not any colon-containing tag', async () => {
  const dom = loadPatchedDom();
  const { document } = dom.window;

  const container = document.createElement('div');
  container.className = 'itemTags';
  const anchor = document.createElement('a');
  anchor.textContent = 'Category:Drama';
  container.appendChild(anchor);
  document.body.appendChild(container);

  await nextTick();

  assert.equal(anchor.textContent, 'Category:Drama');
});

test('does not touch tag links outside .itemTags', async () => {
  const dom = loadPatchedDom();
  const { document } = dom.window;

  const container = document.createElement('div');
  container.className = 'someOtherContainer';
  const anchor = document.createElement('a');
  anchor.textContent = 'provider:Netflix';
  container.appendChild(anchor);
  document.body.appendChild(container);

  await nextTick();

  assert.equal(anchor.textContent, 'provider:Netflix');
});

test('strips tags rendered synchronously on initial page load (no mutation needed)', async () => {
  const dom = loadPatchedDom(
    '<div class="itemTags"><a href="#tag=provider%3AApple%20TV">provider:Apple TV</a></div>',
  );
  const anchor = dom.window.document.querySelector('.itemTags a');

  assert.equal(anchor.textContent, 'Apple TV');
});
