# Ax marketing site

Static marketing and agent-facing docs. Not part of the compiler, runtime,
or default workspace build.

```
site/
  index.html          human marketing page
  styles.css
  app.js
  llms.txt            agent entry (relative links)
  docs/
    index.html        agent-first documentation
    llms.txt
    card.md           compact language card
    card.json         same card, structured
    protocol.md       compiler protocol
    api.md            Ax API framework
```

Open `index.html` in a browser, or serve the folder:

```sh
python3 -m http.server --directory site 4173
```

Agents should fetch `llms.txt`, then `docs/card.md` or `docs/card.json`.
