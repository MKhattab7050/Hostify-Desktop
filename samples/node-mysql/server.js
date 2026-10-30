const express = require('express');
const mysql = require('mysql2/promise');

const pool = mysql.createPool({
  host: process.env.DB_HOST || '127.0.0.1',
  port: Number(process.env.DB_PORT || 3306),
  user: process.env.DB_USER,
  password: process.env.DB_PASSWORD,
  database: process.env.DB_NAME,
  connectionLimit: 5,
});
const app = express();
app.use(express.json());

app.get('/', (_req, res) => res.type('html').send('<h1>Hostify Node and MySQL sample</h1><p id="marker">hostify-sample-node-mysql</p>'));
app.get('/health', async (_req, res) => {
  try {
    await pool.execute('SELECT 1');
    res.json({ ok: true, sample: 'node-mysql', database: process.env.DB_NAME });
  } catch (error) {
    res.status(503).json({ ok: false, sample: 'node-mysql', code: error.code });
  }
});
app.post('/notes', async (req, res) => {
  await pool.execute('CREATE TABLE IF NOT EXISTS notes (id INT AUTO_INCREMENT PRIMARY KEY, body VARCHAR(500) NOT NULL)');
  const [result] = await pool.execute('INSERT INTO notes (body) VALUES (?)', [String(req.body.body ?? '').slice(0, 500)]);
  res.status(201).json({ id: result.insertId });
});
app.get('/notes/:id', async (req, res) => {
  const [rows] = await pool.execute('SELECT id, body FROM notes WHERE id = ?', [Number(req.params.id)]);
  if (rows.length === 0) return res.status(404).json({ found: false });
  res.json(rows[0]);
});

app.listen(Number(process.env.PORT || 3000), () => console.log('listening'));
