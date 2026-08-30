import assert from "node:assert/strict";
import test from "node:test";

import {
  DOWNLOAD_URL_LIFETIME_MS,
  downloadBytes,
} from "../app/lib/browser-download.ts";

function harness({ clickError = null } = {}) {
  const events = [];
  const anchor = {
    href: "",
    download: "",
    hidden: false,
    rel: "",
    click() {
      events.push("click");
      if (clickError) throw clickError;
    },
    remove() { events.push("remove"); },
  };
  const environment = {
    createObjectUrl(blob) {
      events.push(["create", blob.type, blob.size]);
      return "blob:copperline-test";
    },
    revokeObjectUrl(url) { events.push(["revoke", url]); },
    createAnchor() { return anchor; },
    appendAnchor(value) {
      assert.equal(value, anchor);
      events.push("append");
    },
    defer(callback, delay) { events.push(["defer", delay]); callback(); },
  };
  return { anchor, environment, events };
}

test("attaches the download anchor and defers object URL cleanup", () => {
  const { anchor, environment, events } = harness();
  downloadBytes(new Uint8Array([1, 2, 3]), "board.3mf", "model/3mf", environment);

  assert.deepEqual(events, [
    ["create", "model/3mf", 3],
    "append",
    "click",
    "remove",
    ["defer", DOWNLOAD_URL_LIFETIME_MS],
    ["revoke", "blob:copperline-test"],
  ]);
  assert.equal(anchor.href, "blob:copperline-test");
  assert.equal(anchor.download, "board.3mf");
  assert.equal(anchor.hidden, true);
  assert.equal(anchor.rel, "noopener");
});

test("cleans up immediately and reports a synthetic-click failure", () => {
  const failure = new Error("download blocked");
  const { environment, events } = harness({ clickError: failure });

  assert.throws(
    () => downloadBytes(new Uint8Array([1]), "board.stl", "model/stl", environment),
    failure,
  );
  assert.deepEqual(events.slice(-2), ["remove", ["revoke", "blob:copperline-test"]]);
  assert.equal(events.some((event) => Array.isArray(event) && event[0] === "defer"), false);
});
