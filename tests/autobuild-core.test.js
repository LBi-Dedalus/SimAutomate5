import test from "node:test";
import assert from "node:assert/strict";
import { buildForTarget, deliverToTarget } from "../src/autobuild-core.js";

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
