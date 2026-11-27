// A small, safe Markdown renderer for agent replies: paragraphs, headings, lists (nested),
// fenced code, block quotes, tables, rules, and **bold** / *italic* / `code` / ~~strike~~ /
// [links](https://…). Everything is HTML-escaped first; only the tags made here are ever emitted.
// Links are https-only and open in the browser through the app (data-url), never in the page.
"use strict";

(function () {
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  function inline(text) {
    const codes = [];
    // code spans first, so nothing inside them is touched
    let s = text.replace(/`([^`\n]+)`/g, (_, c) => `\u0000${codes.push(c) - 1}\u0000`);
    s = esc(s);
    s = s.replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, label, url) => `<a href="#" data-url="${url}">${label}</a>`);
    s = s.replace(/\*\*([^*\n]+?)\*\*/g, "<strong>$1</strong>").replace(/__([^_\n]+?)__/g, "<strong>$1</strong>");
    s = s.replace(/(^|[\s(])\*([^*\s][^*\n]*?)\*(?=[\s).,:;!?]|$)/g, "$1<em>$2</em>");
    s = s.replace(/(^|[\s(])_([^_\s][^_\n]*?)_(?=[\s).,:;!?]|$)/g, "$1<em>$2</em>");
    s = s.replace(/~~([^~\n]+?)~~/g, "<del>$1</del>");
    return s.replace(/\u0000(\d+)\u0000/g, (_, i) => `<code>${esc(codes[+i])}</code>`);
  }

  const LIST = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
  const isTableSep = (l) => /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(l);
  const cells = (l) => l.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => inline(c.trim()));

  function list(lines, i) {
    // collect the block, then build nested lists from indentation
    const items = [];
    while (i < lines.length) {
      const m = LIST.exec(lines[i]);
      if (m) {
        items.push({ indent: m[1].replace(/\t/g, "    ").length, ordered: /\d/.test(m[2]), start: parseInt(m[2], 10), text: m[3] });
        i++;
      } else if (lines[i].trim() && /^\s{2,}/.test(lines[i]) && items.length) {
        items[items.length - 1].text += "\n" + lines[i].trim(); // continuation line
        i++;
      } else break;
    }
    let html = "";
    const stack = [];
    for (const it of items) {
      const tag = it.ordered ? "ol" : "ul";
      while (stack.length && it.indent < stack[stack.length - 1].indent) html += `</li></${stack.pop().tag}>`;
      // same depth but a different kind of list (bullets, then numbers): a new list
      if (stack.length && it.indent === stack[stack.length - 1].indent && stack[stack.length - 1].tag !== tag) html += `</li></${stack.pop().tag}>`;
      const top = stack[stack.length - 1];
      if (!top || it.indent > top.indent) {
        html += `<${tag}${it.ordered && it.start > 1 ? ` start="${it.start}"` : ""}>`;
        stack.push({ indent: it.indent, tag });
      } else html += "</li>";
      html += `<li>${it.text.split("\n").map(inline).join("<br>")}`;
    }
    while (stack.length) html += `</li></${stack.pop().tag}>`;
    return [html, i];
  }

  function render(src) {
    const lines = String(src ?? "").replace(/\r\n?/g, "\n").split("\n");
    let out = "", i = 0, para = [];
    const flush = () => {
      if (para.length) out += `<p>${para.map(inline).join("<br>")}</p>`;
      para = [];
    };
    while (i < lines.length) {
      const l = lines[i];
      const fence = /^\s*(```|~~~)\s*([\w+-]*)\s*$/.exec(l);
      if (fence) {
        flush();
        const body = [];
        i++;
        while (i < lines.length && !lines[i].trim().startsWith(fence[1])) body.push(lines[i++]);
        i++;
        out += `<pre><code>${esc(body.join("\n"))}</code></pre>`;
        continue;
      }
      if (!l.trim()) { flush(); i++; continue; }
      const h = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(l);
      if (h) { flush(); const n = Math.min(4, h[1].length); out += `<h${n}>${inline(h[2])}</h${n}>`; i++; continue; }
      if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(l)) { flush(); out += "<hr>"; i++; continue; }
      if (/^\s*>/.test(l)) {
        flush();
        const q = [];
        while (i < lines.length && /^\s*>/.test(lines[i])) q.push(lines[i++].replace(/^\s*>\s?/, ""));
        out += `<blockquote>${render(q.join("\n"))}</blockquote>`;
        continue;
      }
      if (l.includes("|") && i + 1 < lines.length && isTableSep(lines[i + 1])) {
        flush();
        const head = cells(l);
        i += 2;
        let rows = "";
        while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows += `<tr>${cells(lines[i++]).map((c) => `<td>${c}</td>`).join("")}</tr>`;
        out += `<table><thead><tr>${head.map((c) => `<th>${c}</th>`).join("")}</tr></thead><tbody>${rows}</tbody></table>`;
        continue;
      }
      if (LIST.test(l)) { flush(); const [html, next] = list(lines, i); out += html; i = next; continue; }
      para.push(l.trim());
      i++;
    }
    flush();
    return out;
  }

  window.markdown = render;
})();
