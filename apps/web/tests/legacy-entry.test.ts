import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const page = readFileSync(new URL('../legacy-entry.html', import.meta.url), 'utf8');
const script = page.match(/<script>([\s\S]*?)<\/script>/)![1];

test('pre-cutover email opens on its original origin and other fragments reach Cubie', () => {
  for (const [hash, destination] of [
    ['#magic=old-token', '/signin#magic=old-token'],
    ['#roomInvite=existing', 'https://call.example.test/#roomInvite=existing'],
    ['#accountInvite=existing', 'https://call.example.test/#accountInvite=existing'],
    ['', 'https://call.example.test/'],
  ]) {
    let redirected;
    const location = { hash, replace: (url: string) => { redirected = url; } };
    const document = { querySelector: () => ({ content: 'https://call.example.test' }) };
    runInNewContext(script, { document, location, URLSearchParams });
    assert.equal(redirected, destination);
  }
});
