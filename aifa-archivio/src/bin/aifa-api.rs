//! Server HTTP JSON: aifa-api [--dir D] [--bind 127.0.0.1:8787]
//!
//!   GET  /                      uso
//!   GET  /cerca?q=testo         ricerca nell'archivio (max 100)
//!   GET  /farmaco/<AIC>         scheda di una confezione
//!   GET  /rcp/<AIC>             paragrafi 4.1–4.9 dell'RCP (scarica/estrae il PDF)
//!   GET  /interazioni?f=AIC|nome|principio|ATC&f=...   verifica interazioni (2–20 farmaci)
//!   POST /interazioni           body JSON {"farmaci": ["026089019", "ZITROMAX"]}
use aifa_archivio::{interazioni, *};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};

struct Ctx { dir: PathBuf, agent: ureq::Agent, voci: Vec<Voce>, ddi: Mutex<Option<Arc<interazioni::Ddi>>> }
const MAX_FARMACI: usize = 20;
const VERSIONE: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |k: &str, d: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned().unwrap_or(d.into());
    let dir = PathBuf::from(opt("--dir", &format!("{}/ArchivioAIFA", std::env::var("HOME").unwrap_or_default())));
    let bind = opt("--bind", "127.0.0.1:8787");
    let voci = load_archivio(&dir);
    eprintln!("archivio: {} ({} confezioni)", dir.display(), voci.len());
    let ctx = Arc::new(Ctx { dir, agent: agent(), voci, ddi: Mutex::new(None) });
    let server = Server::http(&bind).expect("bind");
    eprintln!("in ascolto su http://{bind}");
    for req in server.incoming_requests() {
        let ctx = ctx.clone();
        std::thread::spawn(move || handle(req, &ctx));
    }
}

fn handle(mut req: Request, ctx: &Ctx) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let params: Vec<(String, String)> = query.split('&').filter(|p| !p.is_empty()).map(|p| { let (k, v) = p.split_once('=').unwrap_or((p, "")); (decode(k), decode(v)) }).collect();
    let get = |k: &str| params.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let (status, body): (u16, Value) = match (req.method(), path) {
        (Method::Options, _) => (204, Value::Null),
        (Method::Get, "/") => (200, json!({
            "servizio": "aifa-archivio API", "versione": VERSIONE, "confezioni": ctx.voci.len(), "fonti": fonti(ctx),
            "endpoint": { "GET /cerca?q=": "ricerca (nome, principio attivo, titolare, AIC)", "GET /farmaco/<AIC>": "scheda confezione", "GET /rcp/<AIC>": "paragrafi 4.1-4.9 dell'RCP",
                "GET /interazioni?f=<AIC|nome|principio|ATC>&f=...": "verifica interazioni (2-20 farmaci)", "POST /interazioni": "body {\"farmaci\": [\"AIC o nome\", ...]}" }
        })),
        _ if ctx.voci.is_empty() => (503, json!({ "errore": "archivio non caricato: eseguire una sincronizzazione" })),
        (Method::Get, "/cerca") => { let r = cerca_locale(&ctx.voci, &get("q")); (200, json!({ "totale": r.len(), "risultati": r.into_iter().take(100).collect::<Vec<_>>() })) }
        (Method::Get, p) if p.starts_with("/farmaco/") => match ctx.voci.iter().find(|v| v.aic == p[9..]) {
            Some(v) => (200, json!(v)), None => (404, json!({ "errore": "AIC non trovato" })),
        },
        (Method::Get, p) if p.starts_with("/rcp/") => match risolvi(&ctx.voci, &p[5..]).filter(|v| v.sis > 0) {
            None => (404, json!({ "errore": "farmaco non trovato o senza RCP in banca dati" })),
            Some(v) => match interazioni::testo_rcp(&ctx.agent, &ctx.dir, &Doc::from(v)) {
                Err(e) => (502, json!({ "errore": format!("RCP non disponibile: {e}") })),
                Ok(t) => (200, json!({ "aic": v.aic, "nome": v.nome, "principio_attivo": v.principio, "atc": v.atc, "fonte": "AIFA, Riassunto delle Caratteristiche del Prodotto", "rcp_url": pdf_url(&Doc::from(v), "RCP"),
                    "paragrafi": interazioni::sezioni_rcp(&t).into_iter().map(|(n, t)| (n, Value::String(t))).collect::<serde_json::Map<_, _>>() })),
            },
        },
        (Method::Get, "/interazioni") => interazioni_json(ctx, params.iter().filter(|(k, _)| k == "f" || k == "farmaco").map(|(_, v)| v.clone()).collect()),
        (Method::Post, "/interazioni") => {
            let mut s = String::new();
            req.as_reader().read_to_string(&mut s).ok();
            match serde_json::from_str::<Value>(&s) {
                Ok(v) => interazioni_json(ctx, v["farmaci"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()),
                Err(e) => (400, json!({ "errore": format!("JSON non valido: {e}") })),
            }
        }
        _ => (404, json!({ "errore": "endpoint sconosciuto, vedi GET /" })),
    };
    let mut resp = Response::from_string(if body.is_null() { String::new() } else { body.to_string() }).with_status_code(status);
    for (k, v) in [("Content-Type", "application/json; charset=utf-8"), ("Access-Control-Allow-Origin", "*"), ("Access-Control-Allow-Headers", "Content-Type"), ("Access-Control-Allow-Methods", "GET, POST, OPTIONS")] {
        resp.add_header(Header::from_bytes(k, v).unwrap());
    }
    req.respond(resp).ok();
}

/// Risolve un riferimento alla confezione in banca dati, in ordine: AIC a 9 cifre, nome commerciale, AIC6,
/// principio attivo (esatto), codice ATC (esatto), prefisso del nome, principio attivo senza sale (≥ 4 caratteri). A parità, preferisce le confezioni rimborsate (classe A/H).
fn risolvi<'a>(voci: &'a [Voce], r: &str) -> Option<&'a Voce> {
    let r = r.trim();
    let (low, up) = (r.to_lowercase(), r.to_uppercase());
    let cerca = |f: &dyn Fn(&Voce) -> bool| {
        let mut it = voci.iter().filter(|v| v.sis > 0 && f(v));
        let primo = it.next()?;
        Some(it.find(|v| !v.classe.is_empty()).filter(|_| primo.classe.is_empty()).unwrap_or(primo))
    };
    voci.iter().find(|v| v.aic == r)
        .or_else(|| cerca(&|v| v.nome.eq_ignore_ascii_case(r)))
        .or_else(|| r.parse::<u64>().ok().and_then(|n| cerca(&|v| v.aic6 == n)))
        .or_else(|| cerca(&|v| v.principio.eq_ignore_ascii_case(r)))
        .or_else(|| cerca(&|v| v.atc == up && r.len() >= 4))
        .or_else(|| cerca(&|v| r.len() >= 4 && v.nome.to_lowercase().starts_with(&low)))
        .or_else(|| cerca(&|v| r.len() >= 4 && v.principio.to_lowercase().strip_prefix(&low).is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '/']))))
}

/// Fonti dei dati e data dell'ultimo aggiornamento (per l'audit del chiamante).
fn fonti(ctx: &Ctx) -> Value {
    let mtime = |f: &str| std::fs::metadata(ctx.dir.join(f)).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
    let last = state(&ctx.dir)["last_sync"].as_u64();
    json!({
        "archivio": { "fonte": "AIFA open data confezioni_fornitura.csv + liste classe A/H", "aggiornato": last.map(iso) },
        "rcp": { "fonte": "AIFA, Riassunto delle Caratteristiche del Prodotto, paragrafo 4.5 (PDF scaricato da api.aifa.gov.it, rinnovato ogni 90 giorni)" },
        "ddinter": { "fonte": "DDInter 2.0 (ddinter.scbdd.com), uso accademico non commerciale", "aggiornato": mtime("ddinter.csv").map(iso) }
    })
}

/// Epoch → "AAAA-MM-GG" (algoritmo civil_from_days).
fn iso(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{:02}-{:02}", yoe + era * 400 + (m <= 2) as i64, m, d)
}

fn interazioni_json(ctx: &Ctx, refs: Vec<String>) -> (u16, Value) {
    if refs.len() < 2 { return (400, json!({ "errore": "servono almeno 2 farmaci (parametro f ripetuto, oppure POST {\"farmaci\": [...]})", "codice": "pochi_farmaci" })) }
    if refs.len() > MAX_FARMACI { return (400, json!({ "errore": format!("massimo {MAX_FARMACI} farmaci per chiamata"), "codice": "troppi_farmaci" })) }
    let (mut items, mut trovati, mut non_trovati) = (vec![], vec![], vec![]);
    for r in &refs {
        match risolvi(&ctx.voci, r) {
            Some(v) => { if !items.iter().any(|i: &interazioni::Item| i.pdf_aic6 == v.pdf_aic6) { items.push(interazioni::Item { nome: v.nome.clone(), principio: v.principio.clone(), atc: v.atc.clone(), pdf_sis: v.pdf_sis, pdf_aic6: v.pdf_aic6 }) }
                         trovati.push(json!({ "richiesto": r, "aic": v.aic, "nome": v.nome, "principio_attivo": v.principio, "atc": v.atc, "titolare": v.titolare })) }
            None => non_trovati.push(r.clone()),
        }
    }
    if items.len() < 2 { return (404, json!({ "errore": "meno di 2 farmaci riconosciuti", "codice": "farmaci_non_trovati", "farmaci": trovati, "non_trovati": non_trovati })) }
    let ddi = { let mut g = ctx.ddi.lock().unwrap(); if g.is_none() { *g = interazioni::carica_ddinter(&ctx.dir).map(Arc::new) } g.clone() };
    let rep = interazioni::verifica(&ctx.agent, &ctx.dir, ddi.as_deref(), &items);
    let lvl = |l: &str| match l { "Major" => "grave", "Moderate" => "moderata", "Minor" => "lieve", _ => "non classificata" };
    (200, json!({
        "versione": VERSIONE, "fonti": fonti(ctx),
        "farmaci": trovati, "non_trovati": non_trovati,
        "ddinter": { "disponibile": rep.ddinter_disponibile, "non_mappati": rep.non_mappati,
            "interazioni": rep.ddinter.iter().map(|h| json!({ "a": h.a, "b": h.b, "nome_a": h.nome_a, "nome_b": h.nome_b, "livello": h.livello, "gravita": lvl(&h.livello) })).collect::<Vec<_>>() },
        "rcp": { "interazioni": rep.rcp, "paragrafi_4_5": rep.sezioni.iter().map(|s| json!({ "farmaco": s.nome, "testo": s.testo, "errore": s.errore, "nomi_ddinter": s.inglese })).collect::<Vec<_>>() },
        "scala_gravita": ["grave", "moderata", "lieve", "non classificata"],
        "avvertenza": "Confronto testuale sugli RCP (paragrafo 4.5) e database accademico DDInter (uso non commerciale): non sostituiscono la valutazione clinica."
    }))
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 => { out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%')); i += 2 }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
