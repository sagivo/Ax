const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function copyText(text, button) {
  navigator.clipboard.writeText(text).then(() => {
    const prev = button.textContent;
    button.textContent = "copied";
    button.classList.add("copied");
    window.setTimeout(() => {
      button.textContent = prev;
      button.classList.remove("copied");
    }, 1400);
  });
}

document.querySelectorAll("[data-copy]").forEach((button) => {
  button.addEventListener("click", () => {
    const sel = button.getAttribute("data-copy");
    const node = sel ? document.querySelector(sel) : button.previousElementSibling;
    if (!node) return;
    copyText(node.innerText.trim(), button);
  });
});

function paintListing(node, text, caret) {
  const frag = document.createDocumentFragment();
  let buf = "";
  const flush = () => {
    if (!buf) return;
    frag.append(buf);
    buf = "";
  };
  for (const ch of text) {
    if (ch === "#" || ch === "=") {
      flush();
      const glyph = document.createElement("span");
      glyph.className = "glyph";
      glyph.textContent = ch;
      frag.append(glyph);
    } else {
      buf += ch;
    }
  }
  flush();
  if (caret) {
    const mark = document.createElement("span");
    mark.className = "caret";
    mark.setAttribute("aria-hidden", "true");
    frag.append(mark);
  }
  node.replaceChildren(frag);
}

const program = document.querySelector("[data-type]");
if (program) {
  const source = program.getAttribute("data-type");
  if (reduced) {
    paintListing(program, source, false);
  } else {
    const lines = source.split("\n");
    program.textContent = "";
    let line = 0;
    let col = 0;
    const tick = () => {
      if (line >= lines.length) {
        paintListing(program, lines.join("\n"), false);
        return;
      }
      const shown = lines.slice(0, line).concat(lines[line].slice(0, col)).join("\n");
      paintListing(program, shown, true);
      col += 1;
      if (col > lines[line].length) {
        line += 1;
        col = 0;
        window.setTimeout(tick, 90);
      } else {
        window.setTimeout(tick, 22);
      }
    };
    tick();
  }
}
