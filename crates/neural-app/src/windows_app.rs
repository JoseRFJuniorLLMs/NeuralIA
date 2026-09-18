use std::thread;

use neural_core::{
    google_ai_url, home_html, parse_intent, reader_html, CoreConfig, HistoryEntry, HistoryKind,
    HistoryStore, Intent, ReaderArticle, ReaderClient,
};
use serde_json::Value;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    window::{Window, WindowId},
};
use wry::{WebView, WebViewBuilder};

enum UserEvent {
    Ipc(String),
    ReaderReady(Result<ReaderArticle, String>),
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Window>,
    webview: Option<WebView>,
    config: CoreConfig,
    history: HistoryStore,
}

impl App {
    fn new(proxy:EventLoopProxy<UserEvent>)->Self{
        let config=CoreConfig::default();
        let history=HistoryStore::new(config.data_dir.join("history.jsonl"));
        Self{proxy,window:None,webview:None,config,history}
    }
    fn show_home(&self){
        if let Some(webview)=&self.webview{ let _=webview.load_html(&home_html()); }
    }
    fn handle_input(&mut self,input:String){
        match parse_intent(&input){
            Ok(Intent::Home)=>self.show_home(),
            Ok(Intent::Ask(query))=>self.ask(query),
            Ok(Intent::Read(url))=>self.read(url.to_string()),
            Ok(Intent::Web(url))=>self.web(url.to_string()),
            Err(error)=>self.show_error(&error.to_string()),
        }
    }
    fn ask(&mut self,query:String){
        match google_ai_url(&query,&self.config.language){
            Ok(url)=>{
                self.record(HistoryKind::Ask,query,url.to_string());
                if let Some(webview)=&self.webview{
                    if let Err(error)=webview.load_url(url.as_str()){ self.show_error(&error.to_string()); }
                }
            }
            Err(error)=>self.show_error(&error.to_string()),
        }
    }
    fn read(&mut self,url:String){
        self.record(HistoryKind::Read,url.clone(),url.clone());
        let proxy=self.proxy.clone();
        let timeout=self.config.reader_timeout_secs;
        let max_bytes=self.config.reader_max_bytes;
        thread::spawn(move||{
            let result=ReaderClient::new(timeout,max_bytes).fetch(&url).map_err(|e|e.to_string());
            let _=proxy.send_event(UserEvent::ReaderReady(result));
        });
    }
    fn web(&mut self,url:String){
        match neural_core::validate_web_url(&url){
            Ok(valid)=>{
                self.record(HistoryKind::Web,valid.to_string(),valid.to_string());
                if let Some(webview)=&self.webview{
                    if let Err(error)=webview.load_url(valid.as_str()){ self.show_error(&error.to_string()); }
                }
            }
            Err(error)=>self.show_error(&error.to_string()),
        }
    }
    fn record(&self,kind:HistoryKind,input:String,target:String){
        let _=self.history.append(&HistoryEntry::now(kind,input,target));
    }
    fn show_error(&self,message:&str){
        if let Some(webview)=&self.webview{
            let article=ReaderArticle{source_url:"neural:error".into(),title:"Não foi possível concluir a ação".into(),byline:None,excerpt:Some(message.into()),blocks:vec![]};
            let _=webview.load_html(&reader_html(&article));
        }
    }
    fn handle_ipc(&mut self,raw:String){
        let Ok(value)=serde_json::from_str::<Value>(&raw) else { return; };
        let action=value.get("action").and_then(Value::as_str).unwrap_or("");
        let input=value.get("value").and_then(Value::as_str).unwrap_or("").trim().to_string();
        match action{
            "home"=>self.show_home(),
            "go"=>self.handle_input(input),
            "ask" if !input.is_empty()=>self.ask(input),
            "read" if !input.is_empty()=>{
                self.handle_input(if input.starts_with("reader:"){input}else{format!("reader:{input}")});
            }
            "web" if !input.is_empty()=>{
                self.handle_input(if input.starts_with("web:"){input}else{format!("web:{input}")});
            }
            _=>{}
        }
    }
}

impl ApplicationHandler<UserEvent> for App{
    fn resumed(&mut self,event_loop:&ActiveEventLoop){
        if self.window.is_some(){ return; }
        let attributes=Window::default_attributes()
            .with_title("NeuralIA")
            .with_inner_size(LogicalSize::new(1120.0,760.0))
            .with_min_inner_size(LogicalSize::new(760.0,520.0));
        let window=match event_loop.create_window(attributes){
            Ok(window)=>window,
            Err(error)=>{eprintln!("window creation failed: {error}");event_loop.exit();return;}
        };
        let proxy=self.proxy.clone();
        let webview=WebViewBuilder::new()
            .with_html(home_html())
            .with_initialization_script(EXTERNAL_PAGE_BRIDGE)
            .with_ipc_handler(move|request|{let _=proxy.send_event(UserEvent::Ipc(request.body().clone()));})
            .build(&window);
        match webview{
            Ok(webview)=>{self.webview=Some(webview);self.window=Some(window);}
            Err(error)=>{eprintln!("WebView2 initialization failed: {error}");event_loop.exit();}
        }
    }

    fn user_event(&mut self,_event_loop:&ActiveEventLoop,event:UserEvent){
        match event{
            UserEvent::Ipc(raw)=>self.handle_ipc(raw),
            UserEvent::ReaderReady(Ok(article))=>{
                if let Some(webview)=&self.webview{let _=webview.load_html(&reader_html(&article));}
            }
            UserEvent::ReaderReady(Err(error))=>self.show_error(&error),
        }
    }

    fn window_event(&mut self,event_loop:&ActiveEventLoop,_window_id:WindowId,event:WindowEvent){
        match event{
            WindowEvent::CloseRequested=>event_loop.exit(),
            WindowEvent::KeyboardInput{event,..}=>{
                if event.state.is_pressed()
                    && matches!(event.logical_key,winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)){
                    self.show_home();
                }
            }
            _=>{}
        }
    }
}

pub fn run()->Result<(),Box<dyn std::error::Error>>{
    let event_loop=EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy=event_loop.create_proxy();
    let mut app=App::new(proxy);
    event_loop.run_app(&mut app)?;
    Ok(())
}

const EXTERNAL_PAGE_BRIDGE:&str=r#"
document.addEventListener('DOMContentLoaded', () => {
  if (document.getElementById('neural-shell') || document.getElementById('neuralia-return')) return;
  const b = document.createElement('button');
  b.id = 'neuralia-return';
  b.textContent = '◀ NeuralIA';
  Object.assign(b.style, {position:'fixed',left:'16px',bottom:'16px',zIndex:'2147483647',
    border:'0',borderRadius:'999px',padding:'11px 16px',background:'#111314',color:'#fff',
    font:'600 13px Segoe UI, sans-serif',boxShadow:'0 6px 24px rgba(0,0,0,.25)',cursor:'pointer'});
  b.addEventListener('click', () => window.ipc.postMessage(JSON.stringify({action:'home',value:''})));
  document.documentElement.appendChild(b);
});
"#;
