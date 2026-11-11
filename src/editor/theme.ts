import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";

export const quillHighlight = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword, t.definitionKeyword], color: "var(--syn-keyword)" },
  { tag: [t.string, t.special(t.string), t.regexp], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--syn-number)" },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--syn-function)" },
  { tag: [t.typeName, t.className, t.namespace], color: "var(--syn-type)" },
  { tag: [t.propertyName, t.attributeName], color: "var(--syn-property)" },
  { tag: [t.comment, t.docComment], color: "var(--syn-comment)", fontStyle: "italic" },
  { tag: [t.operator, t.punctuation, t.bracket], color: "var(--syn-punct)" },
  { tag: [t.tagName], color: "var(--syn-keyword)" },
  { tag: [t.heading], color: "var(--syn-function)", fontWeight: "600" },
  { tag: t.strong, fontWeight: "700" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.link, color: "var(--syn-function)", textDecoration: "underline" },
  { tag: t.invalid, color: "var(--del)" },
]);

export const quillTheme = [
  EditorView.theme({
    "&": { height: "100%", backgroundColor: "var(--bg)", color: "var(--text)", fontSize: "13px" },
    ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "1.65", overflow: "auto", scrollBehavior: "smooth" },
    ".cm-content": { padding: "10px 0 40vh", caretColor: "var(--accent)" },
    ".cm-line": { padding: "0 18px 0 6px" },
    ".cm-gutters": { backgroundColor: "var(--bg)", color: "var(--faint)", border: "none", paddingLeft: "6px" },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px 0 8px", minWidth: "34px" },
    ".cm-activeLine": { backgroundColor: "var(--line-active)" },
    ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--dim)" },
    ".cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, ::selection": {
      backgroundColor: "var(--selection) !important",
    },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px", borderRadius: "1px" },
    "&.cm-focused": { outline: "none" },
    ".cm-matchingBracket": { backgroundColor: "var(--accent-soft)", outline: "1px solid var(--accent-line)" },
    ".cm-tooltip": {
      backgroundColor: "var(--elev)", border: "1px solid var(--border-strong)", borderRadius: "10px",
      boxShadow: "var(--shadow-pop)", overflow: "hidden",
    },
    ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "var(--accent-soft)", color: "var(--text)" },
    ".cm-searchMatch": { backgroundColor: "var(--accent-soft)", outline: "1px solid var(--accent-line)" },
    ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--text)", borderTop: "1px solid var(--border)" },
    ".cm-gitgutter": { width: "4px", marginRight: "2px" },
  }),
  syntaxHighlighting(quillHighlight),
];
