//! App desktop (Tauri): backend Rust, interfaccia in ui/index.html.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use aifa_archivio::*;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

struct Ctx { dir: PathBuf, agent: ureq::Agent, syncing: AtomicBool, voci: Mutex<Vec<Voce>>, ddi: Mutex<Option<Arc<interazioni::Ddi>>> }

#[tauri::command]
fn cerca_locale(ctx: State<Ctx>, query: String) -> Vec<Voce> { aifa_archivio::cerca_locale(&ctx.voci.lock().unwrap(), &query) }

#[tauri::command]
fn cerca_online(ctx: State<Ctx>, query: String) -> Result<Vec<Farmaco>, String> { search(&ctx.agent, &query) }

#[tauri::command]
fn apri_pdf(ctx: State<Ctx>, doc: Doc, tipo: String) -> Result<String, String> {
    let p = fetch_pdf(&ctx.agent, &ctx.dir, &doc, &tipo, 90)?;
    open_file(&p)?;
    Ok(p.display().to_string())
}

#[tauri::command(async)]
fn sezione_interazioni(ctx: State<Ctx>, doc: Doc) -> Result<String, String> {
    let t = interazioni::testo_rcp(&ctx.agent, &ctx.dir, &doc)?;
    interazioni::sezione_45(&t).ok_or_else(|| "paragrafo 4.5 non trovato nell'RCP".to_string())
}

#[tauri::command(async)]
fn verifica_terapia(ctx: State<Ctx>, items: Vec<interazioni::Item>) -> interazioni::Report {
    let ddi = {
        let mut g = ctx.ddi.lock().unwrap();
        if g.is_none() { *g = interazioni::carica_ddinter(&ctx.dir).map(Arc::new) }
        g.clone()
    };
    interazioni::verifica(&ctx.agent, &ctx.dir, ddi.as_deref(), &items)
}

#[tauri::command]
fn archivio(ctx: State<Ctx>) -> Value {
    let mut st = state(&ctx.dir);
    if st.is_null() { st = json!({}) }
    st["dir"] = json!(ctx.dir.display().to_string());
    st["now"] = json!(now());
    st["syncing"] = json!(ctx.syncing.load(Ordering::SeqCst));
    st["caricate"] = json!(ctx.voci.lock().unwrap().len());
    st
}

#[tauri::command]
fn salva_impostazioni(ctx: State<Ctx>, every: u64, with_pdf: bool) {
    let mut st = state(&ctx.dir);
    if st.is_null() { st = json!({}) }
    st["settings"] = json!({ "every": every, "with_pdf": with_pdf });
    save_state(&ctx.dir, &st);
}

#[tauri::command]
fn aggiorna(app: AppHandle, ctx: State<Ctx>) -> Result<(), String> {
    if ctx.syncing.swap(true, Ordering::SeqCst) { return Err("aggiornamento già in corso".into()) }
    let st = state(&ctx.dir);
    let o = Opts { dir: ctx.dir.clone(), force: true, no_pdf: !st["settings"]["with_pdf"].as_bool().unwrap_or(false), ..Opts::default() };
    let log: Log = { let app = app.clone(); Arc::new(move |m| { app.emit("sync-log", m).ok(); }) };
    std::thread::spawn(move || {
        sync(&o, log);
        let ctx = app.state::<Ctx>();
        *ctx.voci.lock().unwrap() = load_archivio(&ctx.dir);
        *ctx.ddi.lock().unwrap() = None;
        ctx.syncing.store(false, Ordering::SeqCst);
        app.emit("sync-done", ()).ok();
    });
    Ok(())
}

fn main() {
    let dir = std::env::args().nth(1).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("ArchivioAIFA"));
    let voci = Mutex::new(load_archivio(&dir));
    tauri::Builder::default()
        .manage(Ctx { dir, agent: agent(), syncing: AtomicBool::new(false), voci, ddi: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![cerca_locale, cerca_online, apri_pdf, archivio, salva_impostazioni, aggiorna, sezione_interazioni, verifica_terapia])
        .run(tauri::generate_context!())
        .expect("avvio Tauri");
}
