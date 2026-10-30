#![cfg(windows)]
use hostify_engine::{state::{Engine,Store},pipe};
use hostify_core::deploy::SiteRuntime;
use async_trait::async_trait;
use serde_json::{Value,json};
use std::sync::{Arc,Mutex};
use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
#[derive(Default)]struct Fake{running:Mutex<std::collections::HashSet<String>>}
#[async_trait]impl SiteRuntime for Fake{
 async fn start_site(&self,id:&str)->Result<bool,String>{Ok(self.running.lock().map_err(|e|e.to_string())?.insert(id.into()))}
 async fn stop_site(&self,id:&str)->Result<(),String>{self.running.lock().map_err(|e|e.to_string())?.remove(id);Ok(())}
 async fn start_tunnel(&self,_:&str)->Result<bool,String>{Ok(true)}
 async fn stop_tunnel(&self)->Result<(),String>{Ok(())}
 async fn site_responds(&self,_:u16)->Result<bool,String>{Ok(true)}
}
async fn call(pipe:&mut BufReader<tokio::net::windows::named_pipe::NamedPipeClient>,id:u64,method:&str,params:Value)->Value{
 let mut request=serde_json::to_vec(&json!({"id":id,"method":method,"params":params})).expect("request");request.push(b'\n');pipe.get_mut().write_all(&request).await.expect("write");let mut line=String::new();loop{line.clear();tokio::time::timeout(std::time::Duration::from_secs(5),pipe.read_line(&mut line)).await.expect("response timeout").expect("read");assert!(!line.is_empty());let response:Value=serde_json::from_str(&line).expect("response JSON");if response.get("event").is_some(){continue}assert_eq!(response["id"],id);assert!(response.get("error").is_none(),"{response}");return response["result"].clone();}
}
#[tokio::test]async fn pipe_lifecycle_with_fake_runtime(){
 let dir=tempfile::tempdir().expect("temp");let source=dir.path().join("source");std::fs::create_dir(&source).expect("source");std::fs::write(source.join("index.html"),"hello").expect("index");let store=Store::open(dir.path().join("data")).expect("store");let runtime=Arc::new(Fake::default());let engine=Engine::new(store.clone(),runtime.clone(),false);
 let name=format!(r"\\.\pipe\hostify-test-{}",uuid::Uuid::new_v4());let (tx,rx)=tokio::sync::watch::channel(false);let server_name=name.clone();let server=tokio::spawn(async move{pipe::serve(server_name,engine,rx).await});let mut client=None;for _ in 0..100{match tokio::net::windows::named_pipe::ClientOptions::new().open(&name){Ok(c)=>{client=Some(c);break},Err(_)=>tokio::time::sleep(std::time::Duration::from_millis(20)).await}}let mut client=BufReader::new(client.expect("connect"));
 assert_eq!(call(&mut client,1,"ping",json!({})).await,"pong");assert_eq!(call(&mut client,2,"list_sites",json!({})).await,json!([]));
 call(&mut client,3,"subscribe",json!({})).await;
 let site=call(&mut client,4,"add_site",json!({"name":"test","sourcePath":source,"kind":"directory"})).await;let id=site["id"].as_str().expect("id");assert_eq!(site["fileCount"],1);assert_eq!(site["status"],"stopped");assert!(store.root.join("sites").join(id).join("current/index.html").is_file());
 let started=call(&mut client,5,"start_site",json!({"id":id})).await;assert_eq!(started["status"],"running");assert!(runtime.running.lock().expect("lock").contains(id));
 let stopped=call(&mut client,6,"stop_site",json!({"id":id})).await;assert_eq!(stopped["status"],"stopped");assert!(!runtime.running.lock().expect("lock").contains(id));
 assert_eq!(call(&mut client,7,"remove_site",json!({"id":id,"deleteFiles":true})).await,Value::Null);assert_eq!(call(&mut client,8,"list_sites",json!({})).await,json!([]));assert!(!store.root.join("sites").join(id).exists());
 tx.send(true).expect("shutdown");server.await.expect("join").expect("server");
}
