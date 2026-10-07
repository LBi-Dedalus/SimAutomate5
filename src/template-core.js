// Template variables and interpolation. Pure module (no DOM access).
//
// Variables use the {{NAME}} syntax, which is distinct from control tokens such
// as <VT> or <CR>. NOW and CONTROL_ID are automatic: they are resolved ONCE per
// explicit load/send (see makeContext) and the very same resolved string is shown
// in the preview and sent. Every other variable is manual and must have a value.

export const AUTO_VARIABLES = ["NOW", "CONTROL_ID"];

const VARIABLE_RE = /\{\{([A-Za-z_][A-Za-z0-9_]*)\}\}/g;

export function isAutoVariable(name) {
  return AUTO_VARIABLES.includes(name);
}

/** Unique variable names found in a payload, in order of first appearance. */
export function extractVariables(payload) {
  const names = [];
  for (const match of String(payload).matchAll(VARIABLE_RE)) {
    if (!names.includes(match[1])) names.push(match[1]);
  }
  return names;
}

/** Manual variables of a payload with their stored defaults. */
export function syncVariables(payload, existing = []) {
  return extractVariables(payload)
    .filter((name) => !isAutoVariable(name))
    .map((name) => {
      const found = existing.find((variable) => variable.name === name);
      return { name, default: found ? found.default : "" };
    });
}

const pad = (value, length = 2) => String(value).padStart(length, "0");

/** yyyyMMddHHmmss in local time (HL7 timestamp). */
export function formatNow(date) {
  return (
    pad(date.getFullYear(), 4) +
    pad(date.getMonth() + 1) +
    pad(date.getDate()) +
    pad(date.getHours()) +
    pad(date.getMinutes()) +
    pad(date.getSeconds())
  );
}

/** Automatic values, computed once for one load/send. */
export function makeContext(date = new Date()) {
  return { NOW: formatNow(date), CONTROL_ID: String(date.getTime()) };
}

/**
 * Single-pass interpolation (resolved values are never re-interpolated).
 * Returns { text, missing, errors, ok }. Unresolved placeholders stay visible in text.
 */
export function resolveTemplate(payload, manualValues, context) {
  const source = String(payload);
  const missing = [];
  const errors = [];

  const stray = source.replace(VARIABLE_RE, "");
  if (stray.includes("{{") || stray.includes("}}")) {
    errors.push(
      "Malformed placeholder: use {{NAME}} with letters, digits and underscores only.",
    );
  }

  const text = source.replace(VARIABLE_RE, (whole, name) => {
    let value;
    if (isAutoVariable(name)) {
      value = context[name];
    } else {
      value = manualValues[name];
    }
    if (value === undefined || value === "") {
      if (!missing.includes(name)) missing.push(name);
      return whole;
    }
    if (/[\r\n]/.test(value)) {
      const message = `Value of ${name} contains a line break (it would split the frame).`;
      if (!errors.includes(message)) errors.push(message);
      return whole;
    }
    return value;
  });

  if (missing.length > 0) {
    errors.push(`Unresolved variable(s): ${missing.join(", ")}.`);
  }
  return { text, missing, errors, ok: errors.length === 0 };
}

/** Values map of a template's manual variables (their stored defaults). */
export function manualValues(variables) {
  const values = {};
  for (const variable of variables) values[variable.name] = variable.default;
  return values;
}

/** Comparable snapshot of the editable fields. */
export function snapshot(template) {
  return JSON.stringify({
    name: template.name,
    description: template.description,
    payload: template.payload,
    variables: syncVariables(template.payload, template.variables).map((v) => [
      v.name,
      v.default,
    ]),
  });
}

export function newTemplateId(date = new Date(), random = Math.random()) {
  return `tpl-${date.getTime().toString(36)}-${Math.floor(random * 1e8).toString(36)}`;
}
