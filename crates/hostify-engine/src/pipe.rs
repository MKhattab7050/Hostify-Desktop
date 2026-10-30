//! Bounded NDJSON transport, local clients only, protected per-user DACL.
use crate::{state::Engine,windows::Security,validate_request,MAX_REQUEST_BYTES};
use serde_json::{Value,json};use std::{io,sync::Arc};
use tokio::{io::{AsyncBufReadExt,AsyncWriteExt,BufReader},net::windows::named_pipe::{NamedPipeServer,ServerOptions},sync::{watch,broadcast}};
fn listener(name:&str,first:bool)->io::Result<NamedPipeServer>{let security=Security::user_only()?;let mut attr=security.attributes();unsafe{ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create_with_security_attributes_raw(name,(&mut attr as *mut windows_sys::Win32::Security::SECURITY_ATTRIBUTES).cast())}}
pub async fn serve(name:String,engine:Arc<Engine>,mut shutdown:watch::Receiver<bool>)->io::Result<()>{let mut pending=listener(&name,true)?;let mut sessions=tokio::task::JoinSet::new();loop{tokio::select!{_ = shutdown.changed()=>break,result=pending.connect()=>{result?;let next=listener(&name,false)?;let connected=std::mem::replace(&mut pending,next);let engine=engine.clone();let shutdown=shutdown.clone();sessions.spawn(async move{let _=session(connected,engine,shutdown).await;});},Some(_)=sessions.join_next()=>{}}}sessions.abort_all();while sessions.join_next().await.is_some(){}Ok(())}
async fn line<R:tokio::io::AsyncBufRead+Unpin>(reader:&mut R)->io::Result<Option<Vec<u8>>>{let mut out=Vec::new();loop{let chunk=reader.fill_buf().await?;if chunk.is_empty(){if out.is_empty(){return Ok(None)}return Err(io::Error::new(io::ErrorKind::UnexpectedEof,"Request must end with newline"))}let end=chunk.iter().position(|b|*b==b'\n');let n=end.map(|i|i+1).unwrap_or(chunk.len());if out.len()+n>MAX_REQUEST_BYTES+1{return Err(io::Error::new(io::ErrorKind::InvalidData,"Request exceeds the 1 MiB limit"))}out.extend_from_slice(&chunk[..n]);reader.consume(n);if end.is_some(){out.pop();if out.last()==Some(&b'\r'){out.pop();}return Ok(Some(out))}}}
async fn write<W:tokio::io::AsyncWrite+Unpin>(w:&mut W,v:&Value)->io::Result<()>{let mut bytes=serde_json::to_vec(v)?;if bytes.len()>MAX_REQUEST_BYTES{if let Some(id)=v.get("id"){bytes=serde_json::to_vec(&json!({"id":id,"error":"Response exceeds the 1 MiB limit; request fewer records"}))?;}else{return Err(io::Error::other("Event exceeds the 1 MiB limit"))}}bytes.push(b'\n');tokio::time::timeout(std::time::Duration::from_secs(10),w.write_all(&bytes)).await.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"Pipe client is not reading"))??;Ok(())}
async fn session(pipe:NamedPipeServer,engine:Arc<Engine>,mut shutdown:watch::Receiver<bool>)->io::Result<()>{
 let (r,mut w)=tokio::io::split(pipe);let mut reader=BufReader::new(r);let mut events=engine.store.events.subscribe();let mut subscribed=false;
 loop{
  // Keep the read future alive while events arrive: cancelling it would discard
  // the prefix of a request already consumed from the pipe.
  let bytes={let read=line(&mut reader);tokio::pin!(read);loop{tokio::select!{
   _ = shutdown.changed()=>return Ok(()),
   event=events.recv(),if subscribed=>match event{Ok(v)=>write(&mut w,&v).await?,Err(broadcast::error::RecvError::Lagged(_))=>return Err(io::Error::other("Event client fell behind; reconnect and refresh state")),Err(_)=>return Ok(())},
   bytes=&mut read=>break bytes,
  }}};
  let bytes=match bytes{Ok(Some(b))=>b,Ok(None)=>return Ok(()),Err(e)=>{write(&mut w,&json!({"id":0,"error":e.to_string()})).await?;return Ok(())}};
  let response=match validate_request(&bytes){Ok(r)=>{if r.method=="subscribe"{subscribed=true;}let id=r.id;match engine.dispatch(&r.method,r.params).await{Ok(result)=>json!({"id":id,"result":result}),Err(error)=>json!({"id":id,"error":error.to_string()})}},Err(error)=>{let id=serde_json::from_slice::<Value>(&bytes).ok().and_then(|v|v["id"].as_u64()).unwrap_or(0);json!({"id":id,"error":error})}};
  write(&mut w,&response).await?;
 }
}
