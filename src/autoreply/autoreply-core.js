// Auto reply rules: editing model, validation, persisted shape and display helpers.
// Pure module (no DOM access). The backend (auto_reply.rs) re-validates everything on save;
// the checks here only give immediate, per-field feedback.
import {
  extractVariables,
  isAutoVariable,
  makeContext,
  manualValues,
  resolveTemplate,
} from "../templates/template-core.js";

export const MAX_DELAY_MS = 60000;
export const MAX_LITERAL_LEN = 4096;

export const TRIGGERS = [
  { value: "hl7", label: "HL7 message" },
  { value: "astm_frame", label: "ASTM frame" },
  { value: "astm_enq", label: "ASTM ENQ" },
];

export const ACTIONS = [
  { value: "template", label: "Saved template" },
  { value: "literal", label: "Literal text" },
  { value: "hl7_ack", label: "Generated HL7 acknowledgement" },
  { value: "none", label: "No auto reply" },
];

const REQUEST_PLACEHOLDER = "‹request MSH-10›";

export function newRuleId(date = new Date(), random = Math.random()) {
  return `rule-${date.getTime().toString(36)}-${Math.floor(random * 1e8).toString(36)}`;
}

/** Editing model of a rule: every input is kept as typed (strings). */
export function newRule(id = newRuleId()) {
  return {
    id,
    name: "New rule",
    enabled: true,
    trigger: "hl7",
    messageType: "",
    useCondition: false,
    condSegment: "",
    condField: "",
    condOperator: "exact",
    condValue: "",
    action: "template",
    templateId: "",
    literal: "",
    ackType: "ACK",
    ackCode: "AA",
    delay: "0",
  };
}

export function ruleFromPersisted(rule) {
  const base = newRule(rule.id);
  base.name = rule.name;
  base.enabled = Boolean(rule.enabled);
  base.trigger = rule.trigger.type;
  base.messageType = rule.trigger.message_type ?? "";
  if (rule.condition) {
    base.useCondition = true;
    base.condSegment = rule.condition.segment;
    base.condField = String(rule.condition.field);
    base.condOperator = rule.condition.operator;
    base.condValue = rule.condition.value;
  }
  base.action = rule.action.type;
  base.templateId = rule.action.template_id ?? "";
  base.literal = rule.action.text ?? "";
  if (rule.action.type === "hl7_ack") {
    base.ackType = rule.action.message_type;
    base.ackCode = rule.action.code;
  }
  base.delay = rule.action.type === "none" ? "0" : String(rule.delay_ms);
  return base;
}

/** The exact shape stored in config.json (call only on a validated rule). */
export function ruleToPersisted(rule) {
  const trigger =
    rule.trigger === "hl7"
      ? { type: "hl7", message_type: rule.messageType.trim() }
      : { type: rule.trigger };
  const condition =
    rule.trigger === "hl7" && rule.useCondition
      ? {
          segment: rule.condSegment.trim(),
          field: Number(rule.condField),
          operator: rule.condOperator,
          value: rule.condValue,
        }
      : null;
  let action;
  if (rule.action === "none") action = { type: "none" };
  else if (rule.action === "template") action = { type: "template", template_id: rule.templateId };
  else if (rule.action === "literal") action = { type: "literal", text: rule.literal };
  else action = { type: "hl7_ack", message_type: rule.ackType.trim(), code: rule.ackCode.trim() };
  return {
    id: rule.id,
    name: rule.name.trim(),
    enabled: rule.enabled,
    trigger,
    condition,
    action,
    delay_ms: rule.action === "none" ? 0 : Number(rule.delay),
  };
}

/** Comparable snapshot of the whole ordered list (any edit, reorder, add or delete changes it). */
export function snapshotRules(rules) {
  return JSON.stringify(rules);
}

// ── Validation ──────────────────────────────────────────────

const hasControl = (text) => /[\u0000-\u001f\u007f-\u009f]/.test(text);

function patternError(text, what) {
  if (text.length < 1 || text.length > 64) return `${what} must contain 1 to 64 characters.`;
  if (hasControl(text) || /[|\s]/.test(text)) {
    return `${what} must not contain spaces or "|".`;
  }
  return null;
}

/** Problems of a rule as { field, message }; `templates` is the saved library (null if unknown). */
export function validateRule(rule, templates) {
  const errors = [];
  const add = (field, message) => errors.push({ field, message });

  if (rule.name.trim() === "") add("name", "Give the rule a name.");
  else if (rule.name.trim().length > 100) add("name", "The name is limited to 100 characters.");

  if (rule.trigger === "hl7") {
    const problem = patternError(rule.messageType.trim(), "The message type");
    if (problem) add("messageType", problem);
  }

  if (rule.trigger === "hl7" && rule.useCondition) {
    if (!/^[A-Z][A-Z0-9]{2}$/.test(rule.condSegment.trim())) {
      add("condSegment", "The segment is 3 upper-case letters or digits, e.g. QRD.");
    }
    const field = rule.condField.trim();
    if (!/^\d+$/.test(field) || Number(field) < 1 || Number(field) > 999) {
      add("condField", "The field number is between 1 and 999.");
    }
    if (rule.condValue.length < 1 || rule.condValue.length > 256 || hasControl(rule.condValue)) {
      add("condValue", "The condition value must contain 1 to 256 characters.");
    }
  }

  if (rule.action === "none") return errors; // nothing is sent: no template, text, ack or delay to check

  if (rule.action === "template") {
    if (rule.templateId === "") {
      add("templateId", "Choose the template to reply with.");
    } else if (templates) {
      const template = templates.find((t) => t.id === rule.templateId);
      if (!template) {
        add("templateId", "This template no longer exists: choose another one.");
      } else {
        for (const message of templateProblems(template, rule.trigger === "hl7")) {
          add("templateId", message);
        }
      }
    }
  } else if (rule.action === "literal") {
    if (rule.literal.trim() === "") add("literal", "Type the text to send, e.g. <ACK>.");
    else if (new TextEncoder().encode(rule.literal).length > MAX_LITERAL_LEN) {
      add("literal", `The literal response is limited to ${MAX_LITERAL_LEN} bytes.`);
    }
  } else if (rule.action === "hl7_ack") {
    if (rule.trigger !== "hl7") add("action", "A generated HL7 acknowledgement needs an HL7 trigger.");
    const problem = patternError(rule.ackType.trim(), "The acknowledgement type");
    if (problem) add("ackType", problem);
    else if (rule.ackType.includes("*")) add("ackType", 'The acknowledgement type cannot contain "*".');
    if (!/^[A-Z]{2}$/.test(rule.ackCode.trim())) {
      add("ackCode", "The code is 2 upper-case letters, e.g. AA, AE or AR.");
    }
  }

  const delay = rule.delay.trim();
  if (!/^\d+$/.test(delay) || Number(delay) > MAX_DELAY_MS) {
    add("delay", `The delay is a whole number of milliseconds between 0 and ${MAX_DELAY_MS}.`);
  }
  return errors;
}

function valuesFor(template, requestValue) {
  const values = manualValues(template.variables ?? []);
  // The request control id always comes from the received message, never from the template.
  values.REQ_CONTROL_ID = requestValue;
  return values;
}

/** Why a saved template cannot be used as an automatic response (same rules as the backend). */
export function templateProblems(template, requestAvailable) {
  const problems = [];
  if (template.payload.trim() === "") problems.push("The template payload is empty.");
  const resolved = resolveTemplate(
    template.payload,
    valuesFor(template, "REQ"),
    makeContext(),
  );
  problems.push(...resolved.errors);
  if (!requestAvailable && extractVariables(template.payload).includes("REQ_CONTROL_ID")) {
    problems.push("{{REQ_CONTROL_ID}} is only available for HL7 message rules.");
  }
  return problems;
}

/** Read-only preview of what a rule would send (nothing is transmitted). */
export function previewTemplate(template, requestAvailable) {
  const resolved = resolveTemplate(
    template.payload,
    valuesFor(template, REQUEST_PLACEHOLDER),
    makeContext(),
  );
  return { text: resolved.text, problems: templateProblems(template, requestAvailable) };
}

/** Variables of a template and where each value comes from. */
export function variableSources(template) {
  const defaults = manualValues(template.variables ?? []);
  return extractVariables(template.payload).map((name) => {
    if (isAutoVariable(name)) {
      return { name, source: name === "NOW" ? "local time yyyyMMddHHmmss" : "unique per reply" };
    }
    if (name === "REQ_CONTROL_ID") return { name, source: "MSH-10 of the received message" };
    return { name, source: defaults[name] ? `default “${defaults[name]}”` : "no default value" };
  });
}

/** All rules at once: per-rule errors keyed by rule id. */
export function validateRules(rules, templates) {
  const byRule = new Map();
  const ids = new Set();
  for (const rule of rules) {
    const errors = validateRule(rule, templates);
    if (ids.has(rule.id)) errors.push({ field: "name", message: "Duplicate rule id." });
    ids.add(rule.id);
    if (errors.length) byRule.set(rule.id, errors);
  }
  return byRule;
}

// ── Ordering ────────────────────────────────────────────────

/** New array with the rule moved by `delta` positions (clamped). */
export function moveRule(rules, id, delta) {
  const from = rules.findIndex((r) => r.id === id);
  if (from < 0) return rules;
  const to = Math.max(0, Math.min(rules.length - 1, from + delta));
  if (to === from) return rules;
  const next = rules.slice();
  const [rule] = next.splice(from, 1);
  next.splice(to, 0, rule);
  return next;
}

// ── Display ─────────────────────────────────────────────────

export function describeTrigger(rule) {
  if (rule.trigger === "astm_frame") return "ASTM frame";
  if (rule.trigger === "astm_enq") return "ASTM ENQ";
  return `HL7 ${rule.messageType.trim() || "…"}`;
}

export function describeCondition(rule) {
  if (rule.trigger !== "hl7" || !rule.useCondition) return "Any (otherwise)";
  const op = rule.condOperator === "glob" ? "matches" : "=";
  return `${rule.condSegment.trim() || "SEG"}-${rule.condField.trim() || "n"} ${op} ${rule.condValue}`;
}

export function describeAction(rule, templates) {
  if (rule.action === "template") {
    if (rule.templateId === "") return "Template: (none chosen)";
    const template = templates?.find((t) => t.id === rule.templateId);
    if (template) return `Template: ${template.name}`;
    return templates ? "Template: (missing)" : "Template: (library unavailable)";
  }
  if (rule.action === "literal") {
    const text = rule.literal.replace(/\s+/g, " ").trim();
    return `Literal: ${text.length > 28 ? `${text.slice(0, 27)}…` : text || "(empty)"}`;
  }
  if (rule.action === "none") return "No auto reply";
  return `HL7 ${rule.ackType.trim() || "ACK"} ${rule.ackCode.trim()}`.trim();
}

export function describeDelay(rule) {
  if (rule.action === "none") return "—";
  return `${rule.delay.trim() || "0"} ms`;
}
