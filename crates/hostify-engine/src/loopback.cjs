// Patch only the host-less TCP overloads, preserving pipes, handles and explicit hosts.
'use strict';
const net = require('node:net');
const original = net.Server.prototype.listen;
const plain = (a) => {
  const proto = Object.getPrototypeOf(a);
  return proto === Object.prototype || proto === null;
};
net.Server.prototype.listen = function (...args) {
  const a = args[0];
  if (a === undefined || a === null) {
    // listen(), listen(undefined, cb): a random port on every interface unless a host follows.
    if (typeof args[1] !== 'string') args.splice(0, 1, 0, '127.0.0.1');
  } else if (typeof a === 'function') {
    args.unshift(0, '127.0.0.1');
  } else if (typeof a === 'object') {
    // Arrays are Node's internal normalized form; a class instance is a handle.
    if (plain(a) && a.host === undefined && a.path === undefined && a.fd === undefined && a.handle === undefined) {
      args[0] = { ...a, host: '127.0.0.1' };
    }
  } else if (typeof a === 'number' || (typeof a === 'string' && /^\d+$/.test(a))) {
    if (typeof args[1] !== 'string') args.splice(1, 0, '127.0.0.1');
  }
  return Reflect.apply(original, this, args);
};
