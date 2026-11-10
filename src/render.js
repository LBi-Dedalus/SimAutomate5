// Safe DOM rendering of protocol text: everything goes through text nodes.
import { splitTokens } from "./control-chars.js";

const SEGMENT_AT_START = /^[A-Z][A-Z0-9]{2}(?=\|)/;
const LINE_BREAK_TOKENS = new Set(["CR", "LF", "VT"]);

/** Appends text with a line-break opportunity after every "|", so long HL7 lines wrap between fields. */
function appendWrappable(container, text) {
  const pieces = text.split("|");
  pieces.forEach((piece, i) => {
    const last = i === pieces.length - 1;
    const value = last ? piece : `${piece}|`;
    if (value) container.appendChild(document.createTextNode(value));
    if (!last) container.appendChild(document.createElement("wbr"));
  });
}

/** Appends text, wrapping segment names (MSH, PID…) found at a line start in .sg spans. */
function appendText(container, text, atLineStart) {
  const lines = text.split("\n");
  lines.forEach((line, i) => {
    if (i > 0) container.appendChild(document.createTextNode("\n"));
    const match = (i > 0 || atLineStart) && SEGMENT_AT_START.exec(line);
    if (match) {
      const sg = document.createElement("span");
      sg.className = "sg";
      sg.textContent = match[0];
      container.appendChild(sg);
      line = line.slice(match[0].length);
    }
    appendWrappable(container, line);
  });
}

/**
 * Renders content with control tokens shown as small chips.
 * breakAfterCr adds a visual line break after each <CR> (display only).
 * segments highlights HL7 segment names at the start of each segment.
 */
export function renderTokens(
  container,
  content,
  { breakAfterCr = false, segments = false } = {},
) {
  const parts = splitTokens(content);
  let lineStart = true;
  parts.forEach((part, index) => {
    if (part.type === "text") {
      if (segments) appendText(container, part.value, lineStart);
      else appendWrappable(container, part.value);
      lineStart = part.value.endsWith("\n");
      return;
    }
    const chip = document.createElement("span");
    chip.className = "cc";
    chip.textContent = part.value;
    chip.title = `<${part.value}>`;
    container.appendChild(chip);
    lineStart = LINE_BREAK_TOKENS.has(part.value);
    if (breakAfterCr && part.value === "CR" && index < parts.length - 1) {
      container.appendChild(document.createTextNode("\n"));
    }
  });
}

export function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
