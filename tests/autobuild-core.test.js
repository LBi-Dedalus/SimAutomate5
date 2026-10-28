import test from "node:test";
import assert from "node:assert/strict";
import { buildForTarget, deliverToTarget, applyTemplateAutobuild, createComposerAutobuild } from "../src/autobuild/autobuild-core.js";

test("applyTemplateAutobuild: off returns the text and never invokes", async () => {
  const calls = [];
  const deps = { invoke: async (...a) => (calls.push(a), { output: "X" }) };
  assert.equal(await applyTemplateAutobuild(deps, "H|abc", { autobuild: false, noEtb: true }), "H|abc");
  assert.equal(calls.length, 0);
});

test("applyTemplateAutobuild: on invokes with {input, no_etb} and returns the output", async () => {
  const calls = [];
  const deps = { invoke: async (...a) => (calls.push(a), { output: "<VT>built" }) };
  assert.equal(await applyTemplateAutobuild(deps, "MSH|x", { autobuild: true, noEtb: true }), "<VT>built");
  assert.equal(await applyTemplateAutobuild(deps, "H|x", { autobuild: true, noEtb: false }), "<VT>built");
  assert.deepEqual(calls[0], ["auto_build_message_cmd", { req: { input: "MSH|x", no_etb: true } }]);
  assert.deepEqual(calls[1], ["auto_build_message_cmd", { req: { input: "H|x", no_etb: false } }]);
});

test("applyTemplateAutobuild: a backend error propagates", async () => {
  const deps = { invoke: async () => { throw "bad input"; } };
  await assert.rejects(applyTemplateAutobuild(deps, "H|x", { autobuild: true, noEtb: false }), /bad input/);
});

function makeDeps({ active = "s1", sessions = ["s1", "s2"], accept = true } = {}) {
  const state = { active, drafts: {}, calls: [] };
  return {
    state,
    invoke: async (cmd, args) => {
      state.calls.push({ cmd, args });
      return { output: `built:${args.req.input}` };
    },
    activeId: () => state.active,
    hasSession: (id) => sessions.includes(id),
    setDraft: (id, text) => {
      if (!accept) return false;
      state.drafts[id] = text;
      return true;
    },
  };
}

test("deliverToTarget copies stored output to the active session without building", () => {
  const deps = makeDeps();
  const r = deliverToTarget(deps, "stored");
  assert.deepEqual(r, { target: "s1", delivered: true, error: null });
  assert.deepEqual(deps.state.drafts, { s1: "stored" });
  assert.equal(deps.state.calls.length, 0);
});

test("deliverToTarget honours an explicitly captured target", () => {
  const deps = makeDeps();
  deps.state.active = "s2";
  const r = deliverToTarget(deps, "x", "s1");
  assert.equal(r.target, "s1");
  assert.deepEqual(deps.state.drafts, { s1: "x" });
});

test("deliverToTarget reports no session, closed session and refused draft", () => {
  const none = makeDeps({ active: null });
  const r1 = deliverToTarget(none, "x");
  assert.equal(r1.delivered, false);
  assert.match(r1.error, /No session is open/);

  const closed = makeDeps({ active: "gone" });
  const r2 = deliverToTarget(closed, "x");
  assert.equal(r2.delivered, false);
  assert.match(r2.error, /closed/);

  const refused = makeDeps({ accept: false });
  assert.match(deliverToTarget(refused, "x").error, /closed/);
});

test("buildForTarget still captures the target before awaiting", async () => {
  const deps = makeDeps();
  const pending = buildForTarget(deps, "H|a", true, { deliver: true });
  deps.state.active = "s2";
  const r = await pending;
  assert.equal(r.target, "s1");
  assert.equal(r.delivered, true);
  assert.deepEqual(deps.state.drafts, { s1: "built:H|a" });
  assert.deepEqual(deps.state.calls[0].args, { req: { input: "H|a", no_etb: true } });
});

test("buildForTarget without deliver does not touch drafts", async () => {
  const deps = makeDeps();
  const r = await buildForTarget(deps, "x", false, { deliver: false });
  assert.deepEqual(r, { output: "built:x", target: "s1", delivered: false, error: null });
  assert.deepEqual(deps.state.drafts, {});
});

function composerHarness({ text = "H|a", sessions = ["s1", "s2"], invoke } = {}) {
  const s = { active: "s1", field: text, drafts: { s1: text, s2: "other" }, alive: new Set(sessions), errors: [], calls: [] };
  const deps = {
    invoke: invoke ?? (async (cmd, args) => (s.calls.push({ cmd, args }), { output: `<STX>${args.req.input}<ETX>` })),
    activeId: () => s.active,
    hasSession: (id) => s.alive.has(id),
    setDraft: (id, t) => (s.alive.has(id) ? ((s.drafts[id] = t), true) : false),
    getDraft: (id) => s.drafts[id],
    readComposer: () => s.field,
    reportError: (id, m) => s.errors.push([id, m]),
  };
  return { s, deps, ab: createComposerAutobuild(deps) };
}

test("composer autobuild replaces the draft with no_etb false and reports nothing", async () => {
  const { s, ab } = composerHarness();
  assert.equal(await ab.run(), true);
  assert.deepEqual(s.calls, [{ cmd: "auto_build_message_cmd", args: { req: { input: "H|a", no_etb: false } } }]);
  assert.equal(s.drafts.s1, "<STX>H|a<ETX>");
  assert.equal(s.drafts.s2, "other");
  assert.deepEqual(s.errors, []);
});

test("composer autobuild: empty text and backend failure keep the draft and report", async () => {
  const empty = composerHarness({ text: "  " });
  assert.equal(await empty.ab.run(), false);
  assert.equal(empty.s.calls.length, 0);
  assert.match(empty.s.errors[0][1], /empty/);

  const bad = composerHarness({ invoke: async () => { throw "boom"; } });
  assert.equal(await bad.ab.run(), false);
  assert.equal(bad.s.drafts.s1, "H|a");
  assert.deepEqual(bad.s.errors, [["s1", "Autobuild failed: boom"]]);
  assert.equal(bad.ab.isPending("s1"), false);
});

test("composer autobuild: switching session never misdirects the output", async () => {
  const { s, ab } = composerHarness();
  const run = ab.run();
  s.active = "s2";
  s.field = "other";
  assert.equal(await run, true);
  assert.equal(s.drafts.s1, "<STX>H|a<ETX>");
  assert.equal(s.drafts.s2, "other");
});

test("composer autobuild: closed session drops the output silently", async () => {
  const { s, ab } = composerHarness();
  const run = ab.run();
  s.alive.delete("s1");
  assert.equal(await run, false);
  assert.deepEqual(s.errors, []);
});

test("composer autobuild: double click builds once; edits made meanwhile are kept", async () => {
  const { s, ab } = composerHarness();
  const first = ab.run();
  assert.equal(ab.isPending("s1"), true);
  assert.equal(await ab.run(), false);
  s.drafts.s1 = "H|a edited";
  assert.equal(await first, false);
  assert.equal(s.calls.length, 1);
  assert.equal(s.drafts.s1, "H|a edited");
  assert.match(s.errors[0][1], /edited/);
});

test("session.html wires the composer button to msg.autobuild, not navigation", async () => {
  const { readFileSync } = await import("node:fs");
  const html = readFileSync(new URL("../src/session/session.html", import.meta.url), "utf8");
  const button = html.match(/<button[^>]*>[^<]*Autobuild<\/button>/)[0];
  assert.match(button, /data-action="click:msg\.autobuild"/);
  assert.doesNotMatch(button, /data-nav|nav\.go/);
});
