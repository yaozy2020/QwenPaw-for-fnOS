import test from "node:test";
import assert from "node:assert/strict";
import { compareVersion } from "../src/versionPanel.ts";

test("combined pre, post and dev versions preserve PEP 440 fallback ordering", () => {
  const ordered = [
    "2.3.0.dev1",
    "2.3.0a1.dev1",
    "2.3.0a1",
    "2.3.0a1.post1.dev1",
    "2.3.0a1.post1",
    "2.3.0b1",
    "2.3.0b1.post1",
    "2.3.0",
    "2.3.0.post1.dev1",
    "2.3.0.post1",
  ];
  for (let index = 0; index < ordered.length - 1; index += 1) {
    assert.equal(compareVersion(ordered[index], ordered[index + 1]), -1, `${ordered[index]} < ${ordered[index + 1]}`);
    assert.equal(compareVersion(ordered[index + 1], ordered[index]), 1);
  }
  assert.equal(compareVersion("2.3.0.post1", "2.3.0.post1"), 0);
});
