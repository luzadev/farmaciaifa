//! Interazioni tra farmaci: (1) paragrafo 4.5 degli RCP AIFA, (2) database DDInter (uso non commerciale).
use crate::{fetch_pdf, pdf_path, Doc, Log};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::{fs, io::Read};

pub const DDINTER_URL: &str = "https://ddinter.scbdd.com/static/media/download/ddinter_downloads_code_{}.csv";
pub const DDINTER_CODES: [&str; 8] = ["A", "B", "D", "H", "L", "P", "R", "V"];
/// Indice ATC/DDD dell'OMS in inglese (copia aperta), per tradurre i principi attivi.
pub const ATC_URL: &str = "https://raw.githubusercontent.com/fabkury/atcd/master/WHO%20ATC-DDD%202024-07-31.csv";

/// Un farmaco della terapia da verificare.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Item { pub nome: String, pub principio: String, pub atc: String, pub pdf_sis: u64, pub pdf_aic6: u64 }

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub ddinter: Vec<DdiHit>,
    pub rcp: Vec<RcpHit>,
    pub sezioni: Vec<Sezione>,
    pub non_mappati: Vec<String>,
    pub ddinter_disponibile: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct DdiHit { pub a: String, pub b: String, pub nome_a: String, pub nome_b: String, pub livello: String }
#[derive(Clone, Debug, Serialize)]
pub struct RcpHit { pub a: String, pub b: String, pub componente: String, pub testo: String, pub segnale: String }
#[derive(Clone, Debug, Serialize)]
pub struct Sezione { pub nome: String, pub testo: Option<String>, pub errore: Option<String>, pub inglese: Vec<String> }

// ---------- RCP paragrafo 4.5 ----------

/// Testo dell'RCP (estratto dal PDF, in cache accanto al PDF).
pub fn testo_rcp(agent: &ureq::Agent, dir: &Path, d: &Doc) -> Result<String, String> {
    let txt = pdf_path(dir, d, "RCP").with_extension("txt");
    if let Ok(t) = fs::read_to_string(&txt) { return Ok(t) }
    let pdf = fetch_pdf(agent, dir, d, "RCP", 90)?;
    let bytes = fs::read(&pdf).map_err(|e| e.to_string())?;
    let t = pdf_extract::extract_text_from_mem(&bytes).map_err(|e| format!("testo non estraibile: {e}"))?;
    fs::write(&txt, &t).ok();
    Ok(t)
}

/// Paragrafo 4.5 "Interazioni con altri medicinali": dal titolo 4.5 al titolo 4.6 (prima occorrenza).
/// Paragrafi 4.1–4.9 dell'RCP (titolo atteso dopo il numero), nell'ordine in cui compaiono.
pub const PARAGRAFI: [(&str, &str); 9] = [("4.1", "indicazion"), ("4.2", "posologia"), ("4.3", "controindicazion"), ("4.4", "avvertenz"),
    ("4.5", "interazion"), ("4.6", "gravidanza"), ("4.7", "guidare"), ("4.8", "indesiderat"), ("4.9", "sovradosaggio")];

/// Estrae i paragrafi 4.x dal testo dell'RCP: (numero, testo). Ogni paragrafo finisce dove inizia il successivo trovato (o "5." proprietà farmacologiche).
pub fn sezioni_rcp(testo: &str) -> Vec<(String, String)> {
    let norm: String = testo.split_whitespace().collect::<Vec<_>>().join(" ");
    let low = norm.to_lowercase();
    // Titolo vero: parola chiave entro 120 caratteri e nessun rimando ("vedere paragrafo 4.5 ...") nei 15 caratteri precedenti.
    let rimando = |i: usize| { let pre = &low[..i]; let pre = &pre[pre.char_indices().rev().nth(14).map(|(k, _)| k).unwrap_or(0)..]; ["paragraf", "vedere", "par.", "sezion"].iter().any(|k| pre.contains(k)) };
    let trova = |num: &str, kw: &str| low.match_indices(num).find(|(i, _)| !rimando(*i) && low[*i..].chars().take(120).collect::<String>().contains(kw)).map(|(i, _)| i);
    let mut starts: Vec<(&str, usize)> = PARAGRAFI.iter().filter_map(|(n, kw)| trova(n, kw).map(|i| (*n, i))).collect();
    if let Some(i) = trova("5.", "farmacologic") { starts.push(("5", i)) }
    starts.sort_by_key(|(_, i)| *i);
    starts.iter().enumerate().filter(|(_, (n, _))| *n != "5").map(|(k, (n, s))| {
        let e = starts.get(k + 1).map(|(_, e)| *e).unwrap_or(low.len().min(s + 20_000));
        (n.to_string(), norm[*s..e].trim().to_string())
    }).collect()
}

pub fn sezione_45(testo: &str) -> Option<String> {
    sezioni_rcp(testo).into_iter().find(|(n, _)| n == "4.5").map(|(_, t)| t).or_else(|| {
        let norm: String = testo.split_whitespace().collect::<Vec<_>>().join(" ");
        let low = norm.to_lowercase();
        let start = low.find("interazioni con altri medicinali")?;
        let end = low[start..].find("gravidanza").map(|e| start + e).unwrap_or(low.len().min(start + 20_000));
        Some(norm[start..end].trim().to_string())
    })
}

/// Componenti di un principio attivo ("AMOXICILLINA/ACIDO CLAVULANICO" → ["amoxicillina", "clavulanico"]), senza sali.
pub fn componenti(principio: &str) -> Vec<String> {
    const SALI: [&str; 40] = ["cloridrato", "bromidrato", "sodico", "sodica", "sodio", "potassico", "potassio", "calcico", "calcio", "magnesico",
        "mesilato", "maleato", "tartrato", "fumarato", "succinato", "acetato", "diidrato", "monoidrato", "anidro", "anidra", "triidrato", "emiidrato",
        "besilato", "citrato", "fosfato", "solfato", "sale", "di", "e", "acido", "medoxomil", "cilexetil", "etexilato", "umana", "ricombinante",
        "base", "micronizzato", "micronizzata", "idrato", "lattato"];
    principio.to_lowercase().split(|c| c == '/' || c == '+' || c == ',' || c == '(' || c == ')')
        .map(|p| p.split_whitespace().filter(|w| !SALI.contains(w)).collect::<Vec<_>>().join(" "))
        .filter(|p| p.len() >= 4).collect()
}

fn radice(c: &str) -> &str { let n = c.chars().count(); let keep = if n > 7 { n - 2 } else { n.max(4) }; &c[..c.char_indices().nth(keep).map(|(i, _)| i).unwrap_or(c.len())] }

/// Cerca nel paragrafo 4.5 di ogni farmaco i componenti degli altri.
pub fn verifica_rcp(items: &[Item], sezioni: &[Option<String>]) -> Vec<RcpHit> {
    let mut out = vec![];
    for (i, a) in items.iter().enumerate() {
        let Some(sez) = &sezioni[i] else { continue };
        let low = sez.to_lowercase();
        for (j, b) in items.iter().enumerate() {
            if i == j { continue }
            for c in componenti(&b.principio) {
                let r = radice(&c);
                if let Some(pos) = low.find(r) {
                    let (s, e) = (low[..pos].char_indices().rev().nth(180).map(|(k, _)| k).unwrap_or(0), low[pos..].char_indices().nth(260).map(|(k, _)| pos + k).unwrap_or(low.len()));
                    let frase = &low[s..e];
                    // ponytail: classificazione lessicale del brano, non clinica; il testo completo è sempre allegato
                    let segnale = if frase.contains("controindicat") { "controindicazione" } else if ["evitare", "sconsigliat", "non deve essere", "non devono essere", "non raccomandat"].iter().any(|k| frase.contains(k)) { "evitare" } else { "citazione" };
                    out.push(RcpHit { a: a.nome.clone(), b: b.nome.clone(), componente: c, testo: format!("…{}…", sez[s..e].trim()), segnale: segnale.into() });
                    break;
                }
            }
        }
    }
    out
}

// ---------- DDInter ----------

pub struct Ddi { pub nomi: HashSet<String>, pub coppie: HashMap<(String, String), String>, pub atc: HashMap<String, String> }

/// Scarica DDInter (8 file) e l'indice ATC se mancano o sono più vecchi di 30 giorni.
pub fn scarica_ddinter(agent: &ureq::Agent, dir: &Path, log: Log) {
    let get = |url: &str| -> Option<Vec<u8>> { let mut b = vec![]; agent.get(url).call().ok()?.into_reader().read_to_end(&mut b).ok()?; Some(b) };
    let ddi = dir.join("ddinter.csv");
    if !crate::fresh(&ddi, 30) {
        log("scarico DDInter (interazioni, uso non commerciale)".into());
        let mut all = String::new();
        for c in DDINTER_CODES {
            match get(&DDINTER_URL.replace("{}", c)) { Some(b) => all.push_str(&String::from_utf8_lossy(&b)), None => { log(format!("  ERRORE DDInter {c}")); return } }
        }
        fs::write(&ddi, all).ok();
    }
    let atc = dir.join("atc.csv");
    if !crate::fresh(&atc, 30) {
        log("scarico indice ATC (OMS) per la traduzione dei principi attivi".into());
        match get(ATC_URL) { Some(b) => { fs::write(&atc, b).ok(); } None => log("  ERRORE indice ATC".into()) }
    }
}

pub fn carica_ddinter(dir: &Path) -> Option<Ddi> {
    let ddi = fs::read_to_string(dir.join("ddinter.csv")).ok()?;
    let (mut nomi, mut coppie) = (HashSet::new(), HashMap::new());
    for r in crate::parse_csv_sep(&ddi, ',') {
        if r.len() < 5 || r[0] == "DDInterID_A" { continue }
        let (a, b) = (r[1].trim().to_lowercase(), r[3].trim().to_lowercase());
        nomi.insert(a.clone()); nomi.insert(b.clone());
        coppie.insert(if a <= b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) }, r[4].trim().to_string());
    }
    let atc = fs::read_to_string(dir.join("atc.csv")).map(|s| crate::parse_csv_sep(&s, ',').into_iter().filter(|r| r.len() >= 2)
        .map(|r| (r[0].trim().to_string(), r[1].trim().to_lowercase())).collect()).unwrap_or_default();
    Some(Ddi { nomi, coppie, atc })
}

const SINONIMI: [(&str, &str); 22] = [("paracetamol", "acetaminophen"), ("colecalciferol", "cholecalciferol"), ("salbutamol", "albuterol"),
    ("adrenaline", "epinephrine"), ("noradrenaline", "norepinephrine"), ("glibenclamide", "glyburide"), ("ciclosporin", "cyclosporine"),
    ("rifampicin", "rifampin"), ("acetylsalicylic acid", "aspirin"), ("isoprenaline", "isoproterenol"), ("lignocaine", "lidocaine"),
    ("frusemide", "furosemide"), ("pethidine", "meperidine"), ("dicoumarol", "dicumarol"), ("tretinoin", "tretinoin"), ("hyoscine", "scopolamine"),
    ("beclometasone", "beclomethasone"), ("sulfasalazine", "sulfasalazine"), ("thyroxine", "levothyroxine"), ("insulin (human)", "insulin human"),
    ("heparin", "heparin"), ("metamizole", "dipyrone")];

/// Nomi DDInter (inglese) per un principio attivo italiano, via ATC e via regole di traduzione.
pub fn nomi_inglesi(ddi: &Ddi, principio: &str, atc: &str) -> Vec<String> {
    let mut cands: Vec<String> = vec![];
    if let Some(n) = ddi.atc.get(atc) {
        let n: String = n.chars().filter(|c| *c != '(' && *c != ')').collect();
        for p in n.split(" and ").flat_map(|p| p.split(", ")).flat_map(|p| p.split(" with ")).flat_map(|p| p.split('/')) {
            let p = p.split_whitespace().filter(|w| !["sodium", "potassium", "calcium", "medoxomil", "cilexetil", "etexilate", "mesilate", "hydrochloride", "combinations", "comb.", "human"].contains(w)).collect::<Vec<_>>().join(" ");
            if p.len() >= 4 { cands.push(p) }
        }
    }
    for c in componenti(principio) {
        let base = if c.ends_with("ico") || c.ends_with("ica") { format!("{} acid", c) } else { c.clone() };
        let mut v = vec![base.clone()];
        for (a, b) in [("ina", "ine"), ("ina", "in"), ("olo", "ol"), ("ato", "ate"), ("ico", "ic"), ("ano", "ane"), ("ano", "an"), ("ile", "il"), ("ammina", "amine"), ("ide", "ide"), ("one", "one")] {
            if let Some(stem) = base.strip_suffix(a) { v.push(format!("{stem}{b}")) }
            if let Some(stem) = base.strip_suffix(&format!("{a} acid")) { v.push(format!("{stem}{b} acid")) }
        }
        for s in v.clone() {
            for (f, ph) in [(true, false), (false, true), (true, true)] {
                let mut t = s.clone();
                if f { t = t.replace('f', "ph") }
                if ph { t = t.replace("cl", "chl").replace("t", "th").replace("thth", "th") }
                v.push(t);
            }
        }
        cands.extend(v);
    }
    let mut out: Vec<String> = vec![];
    for c in cands {
        let c = SINONIMI.iter().find(|(k, _)| *k == c).map(|(_, v)| v.to_string()).unwrap_or(c);
        let hit = if ddi.nomi.contains(&c) { Some(c.clone()) } else {
            // ponytail: distanza di edit ≤ len/6 copre f/ph, t/th, cl/chl e doppie; falsi positivi rari su nomi ≥ 7 lettere
            let tol = if c.len() >= 7 { (c.len() / 6).max(1) } else { 0 };
            ddi.nomi.iter().filter(|n| n.len().abs_diff(c.len()) <= tol).map(|n| (lev(n, &c), n)).filter(|(d, _)| *d <= tol).min().map(|(_, n)| n.clone())
                .or_else(|| ddi.nomi.iter().find(|n| n.len() >= 5 && c.len() >= 5 && (n.starts_with(&c) || c.starts_with(n.as_str()))).cloned())
        };
        if let Some(h) = hit { if !out.contains(&h) { out.push(h) } }
    }
    out
}

fn lev(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() { cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + (a[i - 1] != b[j - 1]) as usize) }
        prev = cur;
    }
    prev[b.len()]
}

pub fn verifica_ddinter(ddi: &Ddi, items: &[Item], inglesi: &[Vec<String>]) -> Vec<DdiHit> {
    let mut out = vec![];
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            for a in &inglesi[i] {
                for b in &inglesi[j] {
                    let k = if a <= b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
                    if let Some(l) = ddi.coppie.get(&k) {
                        out.push(DdiHit { a: items[i].nome.clone(), b: items[j].nome.clone(), nome_a: a.clone(), nome_b: b.clone(), livello: l.clone() });
                    }
                }
            }
        }
    }
    let rank = |l: &str| match l { "Major" => 0, "Moderate" => 1, "Minor" => 2, _ => 3 };
    out.sort_by_key(|h| rank(&h.livello));
    out
}

/// Verifica completa di una terapia: DDInter + paragrafi 4.5 degli RCP.
pub fn verifica(agent: &ureq::Agent, dir: &Path, ddi: Option<&Ddi>, items: &[Item]) -> Report {
    let mut sezioni = vec![];
    for it in items {
        let d = Doc { pdf_sis: it.pdf_sis, pdf_aic6: it.pdf_aic6, nome: it.nome.clone() };
        let (testo, errore) = match testo_rcp(agent, dir, &d) {
            Ok(t) => match sezione_45(&t) { Some(s) => (Some(s), None), None => (None, Some("paragrafo 4.5 non trovato nell'RCP".into())) },
            Err(e) => (None, Some(e)),
        };
        let inglese = ddi.map(|d| nomi_inglesi(d, &it.principio, &it.atc)).unwrap_or_default();
        sezioni.push(Sezione { nome: it.nome.clone(), testo, errore, inglese });
    }
    let testi: Vec<Option<String>> = sezioni.iter().map(|s| s.testo.clone()).collect();
    let inglesi: Vec<Vec<String>> = sezioni.iter().map(|s| s.inglese.clone()).collect();
    Report {
        rcp: verifica_rcp(items, &testi),
        ddinter: ddi.map(|d| verifica_ddinter(d, items, &inglesi)).unwrap_or_default(),
        non_mappati: items.iter().zip(&inglesi).filter(|(_, e)| e.is_empty()).map(|(i, _)| i.nome.clone()).collect(),
        ddinter_disponibile: ddi.is_some(),
        sezioni,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sezione_e_componenti() {
        let t = "4.4 Avvertenze bla (vedere paragrafo 4.5 “Interazioni con altri medicinali”)\n4.5\nInterazioni con altri medicinali\nIl warfarin aumenta. Antiacidi riducono.\n4.6 Fertilità, gravidanza";
        assert_eq!(sezione_45(t).unwrap(), "4.5 Interazioni con altri medicinali Il warfarin aumenta. Antiacidi riducono.");
        assert_eq!(componenti("AMOXICILLINA/ACIDO CLAVULANICO"), vec!["amoxicillina", "clavulanico"]);
        assert_eq!(componenti("WARFARIN SODICO"), vec!["warfarin"]);
        let items = vec![Item { nome: "A".into(), principio: "AZITROMICINA".into(), ..Default::default() }, Item { nome: "B".into(), principio: "WARFARIN SODICO".into(), ..Default::default() }];
        let hits = verifica_rcp(&items, &[sezione_45(t), None]);
        assert_eq!((hits.len(), hits[0].b.as_str(), hits[0].componente.as_str(), hits[0].segnale.as_str()), (1, "B", "warfarin", "citazione"));
        let sez = sezioni_rcp("4.1 Indicazioni terapeutiche\nTosse.\n4.3 Controindicazioni\nIpersensibilità.\n4.5 Interazioni\nNessuna.\n5. PROPRIETÀ FARMACOLOGICHE\n5.1 bla");
        assert_eq!(sez.iter().map(|(n, t)| format!("{n}={t}")).collect::<Vec<_>>(), ["4.1=4.1 Indicazioni terapeutiche Tosse.", "4.3=4.3 Controindicazioni Ipersensibilità.", "4.5=4.5 Interazioni Nessuna."]);
    }
    #[test]
    fn traduzione() {
        let ddi = Ddi { nomi: ["amoxicillin", "clavulanic acid", "acetaminophen", "chlorphenamine", "methotrexate", "warfarin"].iter().map(|s| s.to_string()).collect(),
            coppie: [(("amoxicillin".to_string(), "warfarin".to_string()), "Moderate".to_string())].into(), atc: [("J01CR02".to_string(), "amoxicillin and beta-lactamase inhibitor".to_string())].into() };
        assert_eq!(nomi_inglesi(&ddi, "AMOXICILLINA/ACIDO CLAVULANICO", "J01CR02"), vec!["amoxicillin", "clavulanic acid"]);
        assert_eq!(nomi_inglesi(&ddi, "PARACETAMOLO", ""), vec!["acetaminophen"]);
        assert_eq!(nomi_inglesi(&ddi, "CLORFENAMINA MALEATO", ""), vec!["chlorphenamine"]);
        assert_eq!(nomi_inglesi(&ddi, "METOTREXATO", ""), vec!["methotrexate"]);
        let items = vec![Item { nome: "Augmentin".into(), ..Default::default() }, Item { nome: "Coumadin".into(), ..Default::default() }];
        let h = verifica_ddinter(&ddi, &items, &[vec!["amoxicillin".into()], vec!["warfarin".into()]]);
        assert_eq!((h.len(), h[0].livello.as_str()), (1, "Moderate"));
    }
}
