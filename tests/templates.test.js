import test from "node:test";
import assert from "node:assert/strict";

import { tokenPayload, splitTokens } from "../src/control-chars.js";
import {
  extractVariables,
  syncVariables,
  resolveTemplate,
  makeContext,
  formatNow,
  snapshot,
} from "../src/template-core.js";

test("tokenPayload maps data-token values to control payloads", () => {
  assert.equal(tokenPayload("VT"), "<VT>");
  assert.equal(tokenPayload("FS.CR"), "<FS><CR>");
  assert.equal(tokenPayload("enq"), "<ENQ>");
  assert.equal(tokenPayload("NOPE"), null);
  assert.equal(tokenPayload("FS.NOPE"), null);
  assert.equal(tokenPayload(""), null);
  assert.equal(tokenPayload(undefined), null);
});

test("splitTokens separates known control tokens only", () => {
  assert.deepEqual(splitTokens("<VT>MSH|a<b>x<CR>"), [
    { type: "ctrl", value: "VT" },
    { type: "text", value: "MSH|a<b>x" },
    { type: "ctrl", value: "CR" },
  ]);
});

test("extractVariables finds unique names and ignores control tokens", () => {
  assert.deepEqual(extractVariables("<VT>{{A}}|{{B}}|{{A}}<CR>"), ["A", "B"]);
  assert.deepEqual(extractVariables("{{ bad }}{{1x}}"), []);
});

test("syncVariables keeps defaults and drops automatic variables", () => {
  const vars = syncVariables("{{NOW}}{{X}}{{Y}}", [
    { name: "X", default: "1" },
    { name: "Gone", default: "z" },
  ]);
  assert.deepEqual(vars, [
    { name: "X", default: "1" },
    { name: "Y", default: "" },
  ]);
});

test("formatNow is yyyyMMddHHmmss", () => {
  assert.equal(formatNow(new Date(2026, 9, 7, 11, 0, 12)), "20261007110012");
});

test("resolveTemplate resolves automatic and manual variables once", () => {
  const ctx = makeContext(new Date(2026, 9, 7, 11, 0, 12));
  const result = resolveTemplate(
    "<VT>MSH|{{NOW}}|{{CONTROL_ID}}<CR>QRD|{{NOW}}|{{ID}}<CR><FS><CR>",
    { ID: "AAZ1" },
    ctx,
  );
  assert.equal(result.ok, true);
  assert.equal(
    result.text,
    `<VT>MSH|20261007110012|${ctx.CONTROL_ID}<CR>QRD|20261007110012|AAZ1<CR><FS><CR>`,
  );
  assert.ok(!result.text.includes("\n"));
});

test("resolveTemplate blocks unresolved, empty and malformed placeholders", () => {
  const ctx = makeContext();
  const missing = resolveTemplate("a{{X}}b", {}, ctx);
  assert.equal(missing.ok, false);
  assert.deepEqual(missing.missing, ["X"]);
  assert.equal(missing.text, "a{{X}}b");

  assert.equal(resolveTemplate("{{X}}", { X: "" }, ctx).ok, false);
  assert.equal(resolveTemplate("{{ X }}", {}, ctx).ok, false);
  assert.equal(resolveTemplate("{{X", {}, ctx).ok, false);
});

test("resolveTemplate does not re-interpolate values and rejects line breaks", () => {
  const ctx = makeContext();
  const result = resolveTemplate("{{A}}|{{B}}", { A: "{{B}}", B: "2" }, ctx);
  assert.equal(result.ok, true);
  assert.equal(result.text, "{{B}}|2");

  const multi = resolveTemplate("{{A}}", { A: "x\ny" }, ctx);
  assert.equal(multi.ok, false);
});

test("resolveTemplate leaves control tokens alone", () => {
  const result = resolveTemplate("<ENQ>", {}, makeContext());
  assert.equal(result.ok, true);
  assert.equal(result.text, "<ENQ>");
});

test("snapshot ignores unused variables so dirty state is stable", () => {
  const base = { name: "n", description: "", payload: "{{X}}", variables: [{ name: "X", default: "1" }] };
  const withExtra = { ...base, variables: [...base.variables, { name: "Old", default: "q" }] };
  assert.equal(snapshot(base), snapshot(withExtra));
  assert.notEqual(snapshot(base), snapshot({ ...base, payload: "{{X}}!" }));
});
