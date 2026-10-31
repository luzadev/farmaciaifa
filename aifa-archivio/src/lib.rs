//! Core: liste AIFA classe A/H (CSV ufficiali), risoluzione codici via API pubblica,
//! archivio locale con Foglio Illustrativo (FI) e Riassunto Caratteristiche Prodotto (RCP).
pub mod interazioni;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, io::Read, thread};

pub const API: &str = "https://api.aifa.gov.it/aifa-bdf-eif-be/1.0.0";
/// Open data AIFA: tutte le confezioni autorizzate con ditta, regime di fornitura e link a FI/RCP (aggiornato ogni giorno).
pub const CONFEZIONI_URL: &str = "https://drive.aifa.gov.it/farmaci/confezioni_fornitura.csv";
pub const LISTE_PAGE: &str = "https://www.aifa.gov.it/liste-dei-farmaci";
/// Ultime liste note, usate se la pagina AIFA non è raggiungibile.
pub const LISTE_DEFAULT: [(&str, &str); 2] = [
    ("A", "https://www.aifa.gov.it/documents/20142/3815901/Classe-A_per_nome-commerciale_30-6-2026.csv"),
    ("H", "https://www.aifa.gov.it/documents/20142/3815901/Classe-H_per_nome-commerciale_30-6-2026.csv"),
];
pub const DAY: u64 = 86_400;
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

pub struct Opts { pub dir: PathBuf, pub every: u64, pub workers: usize, pub pdf_max_age: u64, pub force: bool, pub no_pdf: bool }
impl Default for Opts {
    fn default() -> Self { Opts { dir: ".".into(), every: 7, workers: 8, pdf_max_age: 90, force: false, no_pdf: false } }
}

/// Una confezione: base dall'open data `confezioni_fornitura.csv`, arricchita dalle liste di classe A/H.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Voce {
    pub aic: String,
    pub aic6: u64,
    pub nome: String,          // denominazione del medicinale
    pub denominazione: String, // descrizione della confezione
    pub titolare: String,
    pub stato: String,
    pub procedura: String,
    pub forma: String,
    pub atc: String,
    pub principio: String,
    pub fornitura: String,
    pub sis: u64,              // codice ditta; 0 = confezione non in banca dati (nessun PDF)
    pub pdf_sis: u64,
    pub pdf_aic6: u64,
    // dalle liste A/H (vuoti se la confezione non è rimborsata)
    pub classe: String,
    pub prezzo: String,
    pub gruppo: String,
    pub gruppo_eq: String,
    pub trasparenza: bool,
}

/// Forma/dosaggio dalla ricerca online.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Confezione { pub aic: String, pub nome: String, pub classe: String, pub regime: String, pub stato: String, pub in_commercio: bool }
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Farmaco {
    pub id: String, pub aic6: u64, pub nome: String, pub titolare: String, pub sis: u64, pub forma: String, pub atc: String,
    pub principi: String, pub vie: String, pub confezioni: Vec<Confezione>, pub in_commercio: bool, pub revocato: bool,
    pub tipo_aut: String, pub pdf_sis: u64, pub pdf_aic6: u64,
}

/// Ciò che serve per scaricare un PDF.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Doc { pub pdf_sis: u64, pub pdf_aic6: u64, pub nome: String }
impl From<&Voce> for Doc { fn from(v: &Voce) -> Doc { Doc { pdf_sis: v.pdf_sis, pdf_aic6: v.pdf_aic6, nome: v.nome.clone() } } }
impl From<&Farmaco> for Doc { fn from(f: &Farmaco) -> Doc { Doc { pdf_sis: f.pdf_sis, pdf_aic6: f.pdf_aic6, nome: f.nome.clone() } } }

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(120)).user_agent("Mozilla/5.0 aifa-archivio").build()
}

// ---------- archivio ----------

pub fn state(dir: &Path) -> Value {
    fs::read_to_string(dir.join("state.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}
pub fn save_state(dir: &Path, st: &Value) { fs::create_dir_all(dir).ok(); fs::write(dir.join("state.json"), st.to_string()).ok(); }
pub fn sync_due(o: &Opts) -> bool { o.force || now() >= state(&o.dir)["last_sync"].as_u64().unwrap_or(0) + o.every * DAY }

pub fn load_archivio(dir: &Path) -> Vec<Voce> {
    fs::read_to_string(dir.join("archivio.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Ricerca locale (nome, confezione, principio attivo, titolare, AIC); max 500 risultati.
pub fn cerca_locale(voci: &[Voce], query: &str) -> Vec<Voce> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    if terms.is_empty() { return vec![] }
    voci.iter().filter(|v| {
        let hay = format!("{} {} {} {} {}", v.nome, v.denominazione, v.principio, v.titolare, v.aic).to_lowercase();
        terms.iter().all(|t| hay.contains(t))
    }).take(500).cloned().collect()
}

/// Scarica open data + liste A/H, scrive archivio.json + farmaci.csv, scarica i PDF (se richiesto).
pub fn sync(o: &Opts, log: Log) {
    fs::create_dir_all(o.dir.join("pdf")).expect("mkdir");
    if !sync_due(o) {
        let last = state(&o.dir)["last_sync"].as_u64().unwrap_or(0);
        log(format!("archivio aggiornato {} giorni fa, prossimo sync tra {} giorni", (now() - last) / DAY, (last + o.every * DAY - now()).div_ceil(DAY)));
        return;
    }
    let agent = agent();
    log(format!("scarico open data confezioni: {CONFEZIONI_URL}"));
    let Some(bytes) = get_bytes(&agent, CONFEZIONI_URL) else { log("ERRORE: open data non scaricato, archivio non modificato".into()); return };
    let mut voci = parse_confezioni(&bytes);
    log(format!("  {} confezioni, {} MB", voci.len(), bytes.len() / 1_000_000));
    if voci.is_empty() { log("file vuoto o formato cambiato: archivio non modificato".into()); return }

    let liste = trova_liste(&agent);
    let mut idx: HashMap<String, usize> = voci.iter().enumerate().map(|(i, v)| (v.aic.clone(), i)).collect();
    for (classe, url) in &liste {
        log(format!("scarico lista classe {classe}: {url}"));
        let Some(b) = get_bytes(&agent, url) else { log(format!("  ERRORE: lista classe {classe} non scaricata")); continue };
        let lista = parse_lista(&b, classe);
        let (mut agganciate, mut nuove) = (0, 0);
        for l in lista {
            match idx.get(&l.aic) {
                Some(&i) => { let v = &mut voci[i]; v.classe = l.classe; v.prezzo = l.prezzo; v.gruppo = l.gruppo; v.gruppo_eq = l.gruppo_eq; v.trasparenza = l.trasparenza; agganciate += 1 }
                None => { idx.insert(l.aic.clone(), voci.len()); voci.push(l); nuove += 1 }
            }
        }
        log(format!("  {agganciate} confezioni con prezzo, {nuove} solo in lista (non in banca dati)"));
    }
    voci.sort_by(|a, b| (a.nome.to_lowercase(), &a.aic).cmp(&(b.nome.to_lowercase(), &b.aic)));
    scrivi(o, &voci);
    interazioni::scarica_ddinter(&agent, &o.dir, log.clone());

    if o.no_pdf {
        log("PDF non scaricati (opzione disattivata)".into());
    } else {
        let mut docs: Vec<Doc> = voci.iter().filter(|v| v.sis > 0).map(Doc::from).collect();
        docs.sort_by_key(|d| d.pdf_aic6); docs.dedup_by_key(|d| d.pdf_aic6);
        download_pdfs(&agent, o, &docs, log.clone());
        scrivi(o, &voci);
    }
    let mut st = state(&o.dir);
    if st.is_null() { st = serde_json::json!({}) }
    let mut aic: Vec<u64> = voci.iter().map(|v| v.aic6).collect(); aic.sort_unstable(); aic.dedup();
    st["last_sync"] = serde_json::json!(now());
    st["confezioni"] = serde_json::json!(voci.len());
    st["medicinali"] = serde_json::json!(aic.len());
    st["rimborsate"] = serde_json::json!(voci.iter().filter(|v| !v.classe.is_empty()).count());
    st["liste"] = serde_json::json!(liste);
    save_state(&o.dir, &st);
    log(format!("fatto: {}", o.dir.join("farmaci.csv").display()));
}

/// Open data `confezioni_fornitura.csv` (UTF-8, `;`, campi tra virgolette).
pub fn parse_confezioni(bytes: &[u8]) -> Vec<Voce> {
    let rows = parse_csv(&cp1252(bytes));
    let Some(header) = rows.first() else { return vec![] };
    let col = |name: &str| header.iter().position(|h| h.trim() == name);
    let (Some(i_aic), Some(i_den), Some(i_desc), Some(i_ditta), Some(i_rs), Some(i_rcp)) =
        (col("CODICE_AIC"), col("DENOMINAZIONE"), col("DESCRIZIONE"), col("CODICE_DITTA"), col("RAGIONE_SOCIALE"), col("LINK_RCP")) else { return vec![] };
    let g = |r: &[String], i: Option<usize>| i.and_then(|i| r.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
    let (i_stato, i_proc, i_forma, i_atc, i_pa, i_forn) = (col("STATO_AMMINISTRATIVO"), col("TIPO_PROCEDURA"), col("FORMA"), col("CODICE_ATC"), col("PA_ASSOCIATI"), col("FORNITURA"));
    rows.iter().skip(1).filter_map(|r| {
        let aic = g(r, Some(i_aic));
        if aic.len() != 9 || !aic.bytes().all(|b| b.is_ascii_digit()) { return None }
        let aic6 = aic[..6].parse().ok()?;
        let sis = g(r, Some(i_ditta)).parse().unwrap_or(0);
        // i link puntano a organizzazione/<sis>/farmaci/<aic6>: per gli import paralleli sono quelli del prodotto originale
        let link = g(r, Some(i_rcp));
        let mut it = link.split('/').skip_while(|p| *p != "organizzazione").skip(1);
        let pdf_sis = it.next().and_then(|p| p.parse().ok()).unwrap_or(sis);
        let pdf_aic6 = it.nth(1).and_then(|p| p.parse().ok()).unwrap_or(aic6);
        Some(Voce {
            aic, aic6, sis, pdf_sis, pdf_aic6, nome: g(r, Some(i_den)), denominazione: g(r, Some(i_desc)), titolare: g(r, Some(i_rs)),
            stato: g(r, i_stato), procedura: g(r, i_proc), forma: g(r, i_forma), atc: g(r, i_atc), principio: g(r, i_pa), fornitura: g(r, i_forn),
            ..Default::default()
        })
    }).collect()
}

/// Cerca nella pagina AIFA i link alle liste correnti; se non li trova usa quelli noti.
pub fn trova_liste(agent: &ureq::Agent) -> Vec<(String, String)> {
    let html = get_bytes(agent, LISTE_PAGE).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let mut out = vec![];
    for classe in ["A", "H"] {
        let key = format!("Classe-{classe}_per_nome-commerciale");
        let url = html.match_indices(&key).find_map(|(i, _)| {
            let start = html[..i].rfind("documents/")?;
            let end = i + html[i..].find(".csv")? + 4;
            Some(format!("https://www.aifa.gov.it/{}", &html[start..end]))
        }).unwrap_or_else(|| LISTE_DEFAULT.iter().find(|(c, _)| *c == classe).unwrap().1.to_string());
        out.push((classe.to_string(), url));
    }
    out
}

fn get_bytes(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    for attempt in 0..3 {
        match agent.get(url).call() {
            Ok(r) => { let mut b = Vec::new(); r.into_reader().read_to_end(&mut b).ok()?; return Some(b) }
            Err(ureq::Error::Status(404, _)) => return None,
            Err(_) => thread::sleep(Duration::from_secs(2 << attempt)),
        }
    }
    None
}

/// Decodifica Windows-1252 (le liste AIFA non sono UTF-8).
pub fn cp1252(b: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(b) { return s.to_string() }
    b.iter().map(|&c| match c {
        0x80 => '€', 0x91 => '‘', 0x92 => '’', 0x93 => '“', 0x94 => '”', 0x96 => '–', 0x97 => '—',
        0x81 | 0x8d | 0x8f | 0x90 | 0x9d => '?',
        0x80..=0x9f => '?', _ => c as char,
    }).collect()
}

/// Parser CSV minimale: separatore `;`, campi tra virgolette con `""` e a capo interni.
pub fn parse_csv(s: &str) -> Vec<Vec<String>> { parse_csv_sep(s, ';') }
pub fn parse_csv_sep(s: &str, sep: char) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut field, mut quoted) = (vec![], vec![], String::new(), false);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match (c, quoted) {
            ('"', true) => if it.peek() == Some(&'"') { it.next(); field.push('"') } else { quoted = false },
            ('"', false) if field.is_empty() => quoted = true,
            (c, false) if c == sep => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => { row.push(std::mem::take(&mut field)); rows.push(std::mem::take(&mut row)) }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() { row.push(field); rows.push(row) }
    rows
}

pub fn parse_lista(bytes: &[u8], classe: &str) -> Vec<Voce> {
    let rows = parse_csv(&cp1252(bytes));
    let Some(header) = rows.first() else { return vec![] };
    let col = |pred: &dyn Fn(&str) -> bool| header.iter().position(|h| pred(&h.split_whitespace().collect::<Vec<_>>().join(" ")));
    let i_aic = col(&|h| h == "AIC" || h == "Codice AIC").unwrap_or(5);
    let i_prezzo = col(&|h| h.starts_with("Prezzo al pubblico")).unwrap_or(3);
    let i_tit = col(&|h| h.starts_with("Titolare")).unwrap_or(4);
    let i_geq = col(&|h| h.starts_with("Codice Gruppo")).unwrap_or(6);
    let i_tr = col(&|h| h.starts_with("X="));
    let g = |r: &[String], i: usize| r.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
    rows.iter().skip(1).filter_map(|r| {
        let aic = g(r, i_aic);
        if aic.len() != 9 || !aic.bytes().all(|b| b.is_ascii_digit()) { return None }
        let aic6 = aic[..6].parse().ok()?;
        let den = g(r, 2);
        Some(Voce {
            classe: classe.into(), principio: g(r, 0), gruppo: g(r, 1), nome: den.split('*').next().unwrap_or("").trim().to_string(),
            denominazione: den, prezzo: g(r, i_prezzo), titolare: g(r, i_tit), gruppo_eq: g(r, i_geq),
            trasparenza: i_tr.map(|i| g(r, i) == "X").unwrap_or(false), aic, aic6, pdf_aic6: aic6, ..Default::default()
        })
    }).collect()
}

pub fn q(s: &str) -> String { format!("\"{}\"", s.replace('"', "\"\"")) }

fn scrivi(o: &Opts, voci: &[Voce]) {
    fs::write(o.dir.join("archivio.json"), serde_json::to_string(voci).unwrap()).expect("archivio.json");
    let mut out = String::from("aic;aic6;nome;confezione;titolare;stato;procedura;forma;atc;principio_attivo;fornitura;classe;prezzo_pubblico;gruppo;gruppo_equivalenza;lista_trasparenza;fi_pdf;rcp_pdf;fi_url;rcp_url\n");
    for v in voci {
        let d = Doc::from(v);
        let local = |t: &str| { let p = pdf_path(&o.dir, &d, t); if p.exists() { p.strip_prefix(&o.dir).unwrap_or(&p).display().to_string() } else { String::new() } };
        let url = |t: &str| if v.sis > 0 { pdf_url(&d, t) } else { String::new() };
        out += &[q(&v.aic), v.aic6.to_string(), q(&v.nome), q(&v.denominazione), q(&v.titolare), q(&v.stato), q(&v.procedura), q(&v.forma), q(&v.atc), q(&v.principio),
            q(&v.fornitura), q(&v.classe), q(&v.prezzo), q(&v.gruppo), q(&v.gruppo_eq), (v.trasparenza as u8).to_string(),
            q(&local("FI")), q(&local("RCP")), q(&url("FI")), q(&url("RCP"))].join(";");
        out.push('\n');
    }
    fs::write(o.dir.join("farmaci.csv"), out).expect("farmaci.csv");
}

// ---------- API AIFA ----------

/// Ricerca online per nome, principio attivo, azienda o codice AIC.
pub fn search(agent: &ureq::Agent, query: &str) -> Result<Vec<Farmaco>, String> {
    let r = agent.get(&format!("{API}/formadosaggio/ricerca")).query("query", query).query("page", "0").query("pageSize", "200")
        .call().map_err(|e| e.to_string())?;
    let v: Value = r.into_json().map_err(|e| e.to_string())?;
    Ok(v["data"]["content"].as_array().map(|a| a.iter().filter_map(parse).collect()).unwrap_or_default())
}

fn s(v: &Value) -> String { v.as_str().unwrap_or("").to_string() }
fn join(v: &Value, sep: &str) -> String { v.as_array().map(|a| a.iter().map(s).collect::<Vec<_>>().join(sep)).unwrap_or_default() }
fn num(v: &Value) -> u64 { v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0) }

pub fn parse(d: &Value) -> Option<Farmaco> {
    let m = &d["medicinale"];
    let aic6 = m["aic6"].as_u64()?;
    let sis = m["codiceSis"].as_u64()?;
    let tipo_aut = s(&d["tipoAutorizzazione"]);
    let (pdf_sis, pdf_aic6) = match (tipo_aut.as_str(), d["sisImportazioneParallela"].as_u64(), d["aic6ImportazioneParallela"].as_u64()) {
        ("P", Some(ps), Some(pa)) => (ps, pa),
        _ => (sis, aic6),
    };
    let confezioni: Vec<Confezione> = d["confezioni"].as_array().map(|a| a.iter().map(|c| Confezione {
        aic: s(&c["aic"]), nome: s(&c["denominazionePackage"]), classe: s(&c["classeRimborsabilita"]), regime: join(&c["classiFornitura"], "/"),
        stato: s(&c["descrizioneStatoAmministrativo"]), in_commercio: num(&c["flagCommercio"]) == 1,
    }).collect()).unwrap_or_default();
    Some(Farmaco {
        id: s(&d["id"]), aic6, sis, nome: s(&m["denominazioneMedicinale"]), titolare: s(&m["aziendaTitolare"]), forma: s(&d["descrizioneFormaDosaggio"]),
        atc: join(&d["codiceAtc"], ";"), principi: join(&d["principiAttiviIt"], ";"), vie: join(&d["vieSomministrazione"], ";"),
        in_commercio: confezioni.iter().any(|c| c.in_commercio), revocato: num(&d["revocato"]) == 1, confezioni, tipo_aut, pdf_sis, pdf_aic6,
    })
}

// ---------- PDF ----------

pub fn slug(name: &str) -> String {
    let t: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    t.split('_').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_")
}
pub fn pdf_path(dir: &Path, d: &Doc, tipo: &str) -> PathBuf { dir.join("pdf").join(format!("{}_{}", d.pdf_aic6, slug(&d.nome))).join(format!("{tipo}.pdf")) }
pub fn pdf_url(d: &Doc, tipo: &str) -> String { format!("{API}/organizzazione/{}/farmaci/{}/stampati?ts={tipo}", d.pdf_sis, d.pdf_aic6) }

/// Restituisce il PDF locale (FI o RCP), scaricandolo se manca o è vecchio.
pub fn fetch_pdf(agent: &ureq::Agent, dir: &Path, d: &Doc, tipo: &str, max_age_days: u64) -> Result<PathBuf, String> {
    if d.pdf_sis == 0 { return Err(format!("{} non è stato trovato nell'API AIFA: aggiorna l'archivio", d.nome)) }
    let path = pdf_path(dir, d, tipo);
    if fresh(&path, max_age_days) { return Ok(path) }
    let bytes = get_pdf(agent, &pdf_url(d, tipo)).ok_or_else(|| format!("{tipo} non disponibile su AIFA per {}", d.nome))?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

fn download_pdfs(agent: &ureq::Agent, o: &Opts, docs: &[Doc], log: Log) {
    let jobs: Vec<(PathBuf, String)> = docs.iter().flat_map(|d| ["FI", "RCP"].into_iter().map(move |t| (pdf_path(&o.dir, d, t), pdf_url(d, t))))
        .filter(|(p, _)| !fresh(p, o.pdf_max_age)).collect();
    log(format!("{} PDF da scaricare", jobs.len()));
    let jobs = Arc::new(Mutex::new(jobs));
    let done = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..o.workers).map(|_| {
        let (agent, jobs, done, log) = (agent.clone(), jobs.clone(), done.clone(), log.clone());
        thread::spawn(move || loop {
            let Some((path, url)) = jobs.lock().unwrap().pop() else { break };
            if let Some(bytes) = get_pdf(&agent, &url) { fs::create_dir_all(path.parent().unwrap()).ok(); fs::write(&path, bytes).ok(); }
            let n = done.fetch_add(1, Ordering::SeqCst) + 1;
            if n % 200 == 0 { log(format!("  {n} PDF")) }
        })
    }).collect();
    for h in handles { h.join().unwrap() }
}

pub fn fresh(p: &Path, max_age_days: u64) -> bool {
    fs::metadata(p).and_then(|m| m.modified()).map(|t| t.elapsed().unwrap_or_default().as_secs() < max_age_days * DAY).unwrap_or(false)
}

/// Scarica un PDF; None se il documento non esiste (AIFA risponde 500) o non è un PDF.
fn get_pdf(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    for attempt in 0..3 {
        match agent.get(url).set("Accept", "application/pdf").call() {
            Ok(r) => { let mut b = Vec::new(); r.into_reader().read_to_end(&mut b).ok()?; return b.starts_with(b"%PDF").then_some(b) }
            Err(ureq::Error::Status(c, _)) if c == 404 || c == 500 => return None,
            Err(_) => thread::sleep(Duration::from_secs(2 << attempt)),
        }
    }
    None
}

/// Apre un file con l'applicazione predefinita del sistema.
pub fn open_file(p: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")] let cmd = ("open", vec![p.as_os_str().to_owned()]);
    #[cfg(target_os = "linux")] let cmd = ("xdg-open", vec![p.as_os_str().to_owned()]);
    #[cfg(target_os = "windows")] let cmd = ("cmd", vec!["/C".into(), "start".into(), "".into(), p.as_os_str().to_owned()]);
    std::process::Command::new(cmd.0).args(cmd.1).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn liste_a_e_h() {
        let a = b"Principio attivo;Descrizione gruppo;Denominazione e Confezione;Prezzo al pubblico \x80;Titolare AIC;AIC;Codice Gruppo Equivalenza;X=in lista;Solo in lista di Regione:;Metri cubi\r\n\
                  Amoxicillina;AMOX 875;ABBA*12 cpr;7,90;FIDIA SpA;036816015;CJA;X;;\r\n";
        let h = b"Principio attivo;Descrizione gruppo;Denominazione e Confezione;Prezzo al pubblico \x80;Prezzo Ex-factory \x80;Prezzo massimo \x80;Titolare AIC;\"Codice \nAIC\";Codice Gruppo Equivalenza;Metri cubi\r\n\
                  Abacavir;ABACAVIR 600;ABACAVIR*30 cpr \"X\";170,27;103,16;-;AUROBINDO Srl;044669012;AAD;\r\n";
        let va = parse_lista(a, "A");
        let vh = parse_lista(h, "H");
        assert_eq!((va.len(), vh.len()), (1, 1));
        assert_eq!((va[0].aic.as_str(), va[0].aic6, va[0].prezzo.as_str(), va[0].titolare.as_str(), va[0].trasparenza), ("036816015", 36816, "7,90", "FIDIA SpA", true));
        assert_eq!((vh[0].aic.as_str(), vh[0].prezzo.as_str(), vh[0].titolare.as_str(), vh[0].gruppo_eq.as_str()), ("044669012", "170,27", "AUROBINDO Srl", "AAD"));
        assert_eq!(cp1252(b"Prezzo \x80 caff\xe8"), "Prezzo € caffè");
        assert_eq!(va[0].nome, "ABBA");
        let c = b"\"CODICE_AIC\";\"COD_FARMACO\";\"COD_CONFEZIONE\";\"DENOMINAZIONE\";\"DESCRIZIONE\";\"CODICE_DITTA\";\"RAGIONE_SOCIALE\";\"STATO_AMMINISTRATIVO\";\"TIPO_PROCEDURA\";\"FORMA\";\"CODICE_ATC\";\"PA_ASSOCIATI\";\"FORNITURA\";\"LINK_FI\";\"LINK_RCP\"\n\
                  \"037954070\";\"037954\";\"070\";\"AUGMENTIN\";\"12 CPR\";2937;\"BB FARMA\";\"Autorizzata\";\"Importazione Parallela\";\"Compressa\";\"J01CR02\";\"AMOXICILLINA\";\"RR\";\"x\";\"https://api.aifa.gov.it/aifa-bdf-eif-be/1.0.0/organizzazione/200/farmaci/26089/stampati?ts=RCP\"\n";
        let vc = parse_confezioni(c);
        assert_eq!(vc.len(), 1);
        assert_eq!((vc[0].aic6, vc[0].sis, vc[0].pdf_sis, vc[0].pdf_aic6, vc[0].nome.as_str()), (37954, 2937, 200, 26089, "AUGMENTIN"), "PDF dell'originale per import parallelo");
        assert_eq!(cerca_locale(&va, "abba fidia").len(), 1);
        assert_eq!(cerca_locale(&va, "abba bayer").len(), 0);
    }
    #[test]
    fn import_parallelo() {
        let d: Value = serde_json::json!({"id":"1","tipoAutorizzazione":"P","sisImportazioneParallela":200,"aic6ImportazioneParallela":26089,
            "medicinale":{"aic6":37954,"codiceSis":2937,"denominazioneMedicinale":"AUGMENTIN"},"confezioni":[{"flagCommercio":1}]});
        let f = parse(&d).unwrap();
        assert_eq!((f.pdf_sis, f.pdf_aic6), (200, 26089));
        assert_eq!(slug("Dalacin C / fosfato"), "DALACIN_C_FOSFATO");
    }
}
