import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const page = readFileSync(new URL('../public/legacy-entry.html', import.meta.url), 'utf8');
const script = page.match(/<script>([\s\S]*?)<\/script>/)![1];

test('pre-cutover email opens on its original origin and other fragments reach Cubie', () => {
  for (const [hash, destination] of [
    ['#magic=old-token', '/signin#magic=old-token'],
    ['#roomInvite=existing', 'https://cubiecall.com/#roomInvite=existing'],
    ['#accountInvite=existing', 'https://cubiecall.com/#accountInvite=existing'],
    ['', 'https://cubiecall.com/'],
  ]) {
    let redirected;
    const location = { hash, replace: (url: string) => { redirected = url; } };
    runInNewContext(script, { location, URLSearchParams });
    assert.equal(redirected, destination);
  }
});
