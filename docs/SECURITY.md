# Hostify security and its limits

Hostify makes hosting easier for one person on their own Windows PC. It is not a secure multi-tenant hosting platform, and it must not be described as one.

## Who Hostify protects, and from whom

Imported Node.js and PHP projects are **trusted-user code**. You are choosing to run them, the same as double-clicking a program.

| Threat | Protected? | How |
|---|---|---|
| Someone on the internet reaching your admin controls | Yes | No admin API is exposed. The engine listens only on a named pipe restricted to your Windows account |
| Someone on your network reaching a database | Yes, by default | MySQL binds to `127.0.0.1` and is never put behind a tunnel automatically |
| Someone on your network reaching a Node app directly | Mostly | Apps get `HOST=127.0.0.1` and a preload that makes `listen(port)` use loopback. An app that explicitly binds `0.0.0.0` is allowed, detected and reported |
| Secrets readable at rest by other Windows users | Yes | DPAPI at user scope, Windows Credential Manager |
| Secrets in logs | Reduced | Known secret values are redacted from captured output. An app that transforms a secret before printing it (for example base64) is not caught |
| A malicious or compromised imported app | **No** | See below |
| A malicious npm package | **No** | See below |
| A vulnerable app being attacked from the internet | **No** | The attacker gets whatever the app has. Keep your apps updated |
| One hosted site attacking another | **No** | See below |
| Malware already running as your Windows account | **No** | It can do anything you can, including decrypting DPAPI values |

## What imported code can do

A hosted Node.js or PHP application runs as your Windows user. It can:

- read and change any file you can, including documents and browser data
- read the encrypted environment variables of other sites, by calling DPAPI as you
- connect to other hosted sites on loopback, and to any website
- run other programs, and start programs that outlive it if it breaks out of its Job Object

**Publishing a Node.js or PHP app connects code running on your PC to the internet.** A bug in that app can expose your PC. Only host code you trust, and prefer a dedicated PC or a separate Windows account for anything important.

## Install scripts

npm packages can run programs during installation. Hostify installs with `--ignore-scripts` unless you turn on **Allow install scripts** for that site. Some packages with native code will not work without it. When you do allow them, Hostify withholds variables marked secret from the install step, so a private registry token must be a non-secret variable or an `.npmrc` you accept the risk of. This reduces one risk and removes none of the others: running the app still runs every dependency. It is not a sandbox.

## What Windows Job Objects do here

A Job Object lets Hostify stop a whole process tree and apply a memory limit. It does **not** restrict file, network or registry access. It is process management, not isolation.

## Privilege

Hostify and everything it starts run as the signed-in user, without administrator rights. MySQL runs under the Hostify engine rather than as a Windows service for this reason.

## Database credentials

Each website gets its own MySQL user with rights on its own database only. The administrator password is generated, kept in Credential Manager, and never handed to a website. Least privilege limits what a compromised site can do through SQL. It does not stop that site, which runs as you, from reading the Credential Manager entry or the database files. Remote root login is disabled.

## Downloads

Caddy, cloudflared, Node.js, PHP and MySQL downloads are pinned by SHA-256 inside Hostify, from publisher data. A build that does not match is deleted and never run. New versions arrive through Hostify updates. Packages your projects install from npm are not checked by Hostify.

## Cloudflare

Your API token and tunnel tokens are kept in Credential Manager and are not written to logs. Hostify never asks for your Cloudflare password, and asks before changing DNS.

## Backups

Database backups contain your data in readable SQL. They are stored in your Hostify data folder with access limited to your account. If you copy one elsewhere, protect it yourself.
