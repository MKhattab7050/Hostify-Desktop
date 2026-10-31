//! Built-in, offline-first Hostify App Catalog.
//! Templates are small, original, immutable code samples. No remote downloads,
//! npm install scripts, external shell execution, or silent trust grants.
use serde::Serialize;
use std::{fs,io,path::Path};
use thiserror::Error;

#[derive(Debug,Error)]
pub enum CatalogError {
    #[error("unknown or unsupported template")]
    Unknown,
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Clone,Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub framework: &'static str,
    pub description: &'static str,
    pub requires_trust: bool,
    pub requires_database: bool,
}

const PORTFOLIO: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>My Portfolio</title><style>
:root{color-scheme:dark;font-family:system-ui,sans-serif;background:#10161d;color:#eff5f6}
body{margin:0;min-height:100vh;display:grid;place-items:center}
main{max-width:720px;padding:3rem 1.5rem}p{color:#b6c5cd;line-height:1.7}
h1{font-size:clamp(2.5rem,7vw,5rem);letter-spacing:-.06em;line-height:1}
a{color:#84e2d1}nav{display:flex;gap:1.5rem;flex-wrap:wrap;margin-top:2rem}
</style></head><body><main><p>Available for new projects</p><h1>Ideas worth making.</h1>
<p>Welcome to your self-hosted portfolio. Replace this introduction with your work, achievements, and contact details.</p>
<nav><a href="mailto:hello@example.com">Contact me</a><a href="#projects">View projects</a></nav>
<section id="projects"><h2>Selected work</h2><p>Add your projects here.</p></section>
</main></body></html>"##;

const RESTAURANT: &str = r##"<!doctype html><html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Corner Table | Local Kitchen</title><style>
:root{font-family:Georgia,serif;background:#f7f3ea;color:#263326}
body{margin:0}header,main,footer{max-width:960px;margin:auto;padding:2rem}
header{display:flex;justify-content:space-between;gap:1rem;flex-wrap:wrap}
h1{font-size:clamp(2.5rem,7vw,5.5rem);line-height:1.05;max-width:14ch}
p{font-family:system-ui;line-height:1.7}a{color:#1c6e59}
.hero{padding:4rem 0}ul{padding-left:1.5rem;line-height:2}
</style></head><body>
<header><strong>CORNER TABLE</strong><nav><a href="#menu">Menu</a> · <a href="#visit">Visit</a></nav></header>
<main><section class="hero"><p>Neighborhood kitchen · Fresh every day</p>
<h1>Good food, a better gathering.</h1><p>Welcome to your restaurant's new home on Hostify.</p>
<a href="#menu">Explore the menu</a></section><section id="menu"><h2>Today's favorites</h2>
<ul><li>Seasonal soup</li><li>Roasted vegetables</li><li>Freshly baked bread</li></ul>
</section><section id="visit"><h2>Come by</h2><p>Replace this text with your address, hours and reservations.</p></section>
</main><footer><p>© Corner Table. Make this website your own.</p></footer></body></html>"##;

const NODE_PACKAGE: &str = r#"{"name":"hostify-node-starter","private":true,"version":"1.0.0","scripts":{"start":"node server.cjs"},"engines":{"node":">=24 <27"}}"#;
const NODE_SERVER: &str = r#"// Example API: no external packages, tokens or external network bindings.
const http = require('node:http');
const port = Number(process.env.PORT || 3000);
const server = http.createServer((req, res) => {
  res.setHeader('Content-Type','application/json; charset=utf-8');
  res.setHeader('X-Content-Type-Options','nosniff');
  if (req.method !== 'GET') {res.writeHead(405);res.end('{"error":"method not allowed"}');return;}
  if (req.url === '/healthz') {res.writeHead(200);res.end('{"healthy":true}');return;}
  if (req.url === '/') {res.writeHead(200);res.end(JSON.stringify({message:'Hello from Hostify Node.js'}));return;}
  res.writeHead(404);res.end('{"error":"not found"}');
});
server.listen(port,'127.0.0.1');
process.on('SIGTERM',()=>server.close(()=>process.exit(0)));
"#;

const PHP_INDEX: &str = r#"<?php
declare(strict_types=1);
header('Content-Type: text/html; charset=utf-8');
header('X-Content-Type-Options: nosniff');
?><!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Hostify PHP Starter</title><style>
body{font-family:system-ui;max-width:700px;margin:8vh auto;padding:2rem;line-height:1.7}
h1{font-size:clamp(2rem,6vw,4rem)}
</style></head><body>
<h1>PHP is ready.</h1><p>You're running a self-hosted PHP application through Hostify.</p>
<p>Start building by editing <code>index.php</code> in this generated project folder.</p>
</body></html>"#;

const TEMPLATES: [Template;4] = [
    Template{id:"portfolio",name:"Portfolio",kind:"static",framework:"HTML/CSS",
        description:"Responsive personal portfolio with no dependencies.",requires_trust:false,requires_database:false},
    Template{id:"restaurant",name:"Restaurant Landing",kind:"static",framework:"HTML/CSS",
        description:"A responsive restaurant landing page, ready to customize.",requires_trust:false,requires_database:false},
    Template{id:"node-api",name:"Node.js API",kind:"node",framework:"Node.js 24",
        description:"Small built-in HTTP API with a health endpoint and no dependencies.",requires_trust:true,requires_database:false},
    Template{id:"php-starter",name:"PHP Starter",kind:"php",framework:"PHP 8.4",
        description:"Minimal PHP page without external packages.",requires_trust:true,requires_database:false},
];

pub fn list()->Vec<Template>{ TEMPLATES.to_vec() }
pub fn get(id:&str)->Result<Template,CatalogError>{
    list().into_iter().find(|t|t.id==id).ok_or(CatalogError::Unknown)
}
pub fn materialize(id:&str,target:&Path)->Result<Template,CatalogError>{
    let template=get(id)?;
    let files:&[(&str,&str)]=match id {
        "portfolio"=> &[("index.html",PORTFOLIO)],
        "restaurant"=> &[("index.html",RESTAURANT)],
        "node-api"=> &[("package.json",NODE_PACKAGE),("server.cjs",NODE_SERVER)],
        "php-starter"=> &[("index.php",PHP_INDEX)],
        _ => return Err(CatalogError::Unknown)
    };
    // create_dir refuses to overwrite an existing project. All paths are
    // literal vetted filename constants, never input from the user or network.
    fs::create_dir(target)?;
    let result=(||->Result<(),CatalogError>{
        for (name,content) in files {
            let path=target.join(name);
            fs::write(path,content.as_bytes())?;
        }
        Ok(())
    })();
    if result.is_err(){let _=fs::remove_dir_all(target);}
    result?;
    Ok(template)
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn templates_are_offline_and_do_not_install_or_run_code(){
        let root=tempfile::tempdir().unwrap();
        for template in list(){
            let target=root.path().join(template.id);
            let result=materialize(template.id,&target).unwrap();
            assert_eq!(result.id,template.id);
            assert!(!target.join(".env").exists());
            assert!(!target.join("node_modules").exists());
            if template.kind=="node"{
                let package=fs::read_to_string(target.join("package.json")).unwrap();
                assert!(!package.contains("postinstall"));
                assert!(!package.contains("dependencies"));
                assert!(package.contains("node server.cjs"));
            }
        }
        assert!(materialize("unknown",&root.path().join("evil")).is_err());
        assert!(materialize("portfolio",&root.path().join("portfolio")).is_err());
    }
}
