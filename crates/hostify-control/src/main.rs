use hostify_control::{Controller,serve_request};
use hostify_core::network::Registry;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::{path::PathBuf,sync::Arc,time::Duration};
use tokio::net::TcpListener;

#[tokio::main]
async fn main()->Result<(),Box<dyn std::error::Error>>{
    let token=std::env::var("HOSTIFY_CONTROL_ADMIN_TOKEN")
        .map_err(|_|"Set HOSTIFY_CONTROL_ADMIN_TOKEN to a unique random 32+ byte secret")?;
    let domain=std::env::var("HOSTIFY_CONTROL_BASE_DOMAIN")
        .map_err(|_|"Set HOSTIFY_CONTROL_BASE_DOMAIN to a staging DNS namespace")?;
    let data=std::env::var("HOSTIFY_CONTROL_DATABASE")
        .map(PathBuf::from).map_err(|_|"Set HOSTIFY_CONTROL_DATABASE to a writable SQLite path")?;
    let port:u16=std::env::var("HOSTIFY_CONTROL_PORT").unwrap_or_else(|_|"8787".into()).parse()?;
    if port==0{return Err("Invalid port".into());}
    // Critically, the control plane is NEVER bound to 0.0.0.0 by this binary.
    // A future public API gateway must enforce HTTPS, RBAC, MFA, and rate limits.
    let registry=Registry::open(&data,&domain)?;
    let control=Arc::new(Controller::new(registry,token.into_bytes())?);
    let listener=TcpListener::bind(("127.0.0.1",port)).await?;
    eprintln!("Hostify control plane listening on 127.0.0.1:{port} (LOCAL ONLY).");
    loop{
        let (stream,_)=listener.accept().await?;
        let controller=control.clone();
        tokio::spawn(async move{
            let service=service_fn(move |request|serve_request(request,controller.clone()));
            let conn=http1::Builder::new().serve_connection(TokioIo::new(stream),service);
            // Avoid holding file descriptors and request bodies forever.
            let _=tokio::time::timeout(Duration::from_secs(30),conn).await;
        });
    }
}
