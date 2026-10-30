const express = require('express');
const http = require('node:http');
const path = require('node:path');
const { Server } = require('socket.io');

const app = express();
const server = http.createServer(app);
const io = new Server(server);
const port = Number(process.env.PORT || 3000);

app.use(express.static(path.join(__dirname, 'public')));
app.get('/health', (_req, res) => res.json({ ok: true, sample: 'socketio', clients: io.engine.clientsCount }));

io.on('connection', (socket) => {
  // The transport is reported back so a test can prove the upgrade to WebSocket really happened.
  socket.conn.on('upgrade', (transport) => socket.emit('transport', transport.name));
  socket.emit('transport', socket.conn.transport.name);
  socket.on('chat', (text) => io.emit('chat', String(text).slice(0, 500)));
});

server.listen(port, () => console.log(`listening on ${port}`));
