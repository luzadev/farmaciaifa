//! CLI: aifa-cli sync|watch|interazioni <AIC>... [--dir D] [--every GG] [--workers N] [--pdf-max-age GG] [--force] [--no-pdf]
use aifa_archivio::*;
use std::sync::Arc;
use std::{thread, time::Duration};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("sync");
    let mut o = Opts::default();
    let mut i = 1;
    while i < args.len() {
        let next = |i: &mut usize| -> String { *i += 1; args.get(*i).cloned().unwrap_or_default() };
        match args[i].as_str() {
            "--dir" => o.dir = next(&mut i).into(),
            "--every" => o.every = next(&mut i).parse().expect("--every giorni"),
            "--workers" => o.workers = next(&mut i).parse().expect("--workers N"),
            "--pdf-max-age" => o.pdf_max_age = next(&mut i).parse().expect("--pdf-max-age giorni"),
            "--force" => o.force = true,
            "--no-pdf" => o.no_pdf = true,
            a if a.starts_with("--") => { eprintln!("opzione sconosciuta: {a}"); std::process::exit(2) }
            _ => {}
        }
        i += 1;
    }
    let log: Log = Arc::new(|m| eprintln!("{m}"));
    match cmd {
        "sync" => sync(&o, log),
        "watch" => loop { sync(&o, log.clone()); thread::sleep(Duration::from_secs(3600)) },
        "interazioni" => {
            let voci = load_archivio(&o.dir);
            let items: Vec<interazioni::Item> = args.iter().skip(1).filter(|a| !a.starts_with("--")).filter_map(|aic| voci.iter().find(|v| &v.aic == aic || v.nome.eq_ignore_ascii_case(aic)))
                .map(|v| interazioni::Item { nome: v.nome.clone(), principio: v.principio.clone(), atc: v.atc.clone(), pdf_sis: v.pdf_sis, pdf_aic6: v.pdf_aic6 }).collect();
            let agent = agent();
            interazioni::scarica_ddinter(&agent, &o.dir, log.clone());
            let ddi = interazioni::carica_ddinter(&o.dir);
            let r = interazioni::verifica(&agent, &o.dir, ddi.as_ref(), &items);
            for s in &r.sezioni { println!("{}: inglese={:?} 4.5={} {}", s.nome, s.inglese, s.testo.as_ref().map(|t| t.len()).unwrap_or(0), s.errore.clone().unwrap_or_default()) }
            for h in &r.ddinter { println!("DDInter {:8} {} ({}) + {} ({})", h.livello, h.a, h.nome_a, h.b, h.nome_b) }
            for h in &r.rcp { println!("RCP {} cita {} ({}): {}", h.a, h.b, h.componente, h.testo) }
            if !r.non_mappati.is_empty() { println!("non mappati su DDInter: {:?}", r.non_mappati) }
        }
        _ => { eprintln!("comandi: sync | watch"); std::process::exit(2) }
    }
}
