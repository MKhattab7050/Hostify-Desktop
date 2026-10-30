const express = require('express');
const app = express();
const port = Number(process.env.PORT || 3000);
const startedAt = Date.now();

app.get('/', (_req, res) => res.type('html').send('<h1>Hostify Express sample</h1><p id="marker">hostify-sample-express</p>'));
app.get('/health', (_req, res) => res.json({ ok: true, sample: 'express', pid: process.pid, uptimeMs: Date.now() - startedAt }));
// Reports whether a variable reached the app without ever returning its value.
app.get('/env/:name', (req, res) => res.json({ name: req.params.name, present: process.env[req.params.name] !== undefined }));
app.get('/crash', (_req, res) => {
  res.json({ crashing: true });
  setTimeout(() => process.exit(42), 50);
});

const server = app.listen(port, () => console.log(`listening on ${port}`));
process.on('SIGINT', () => {
  console.log('graceful shutdown');
  server.close(() => process.exit(0));
});
