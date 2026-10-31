# Farmaci AIFA

Gestione locale dell'archivio dei medicinali AIFA: elenco completo delle confezioni, prezzi di classe A e H,
Foglio Illustrativo e Riassunto delle Caratteristiche del Prodotto (RCP) per ogni farmaco, verifica delle
interazioni tra farmaci e API HTTP per collegare altri programmi.

## Contenuto del repository

| Cartella / file | Cosa contiene |
|---|---|
| [`aifa-archivio/`](aifa-archivio/) | il programma, in Rust: app desktop (Tauri 2), riga di comando e server API. Vedi il suo [README](aifa-archivio/README.md) |
| [`aifa-archivio/API.md`](aifa-archivio/API.md) | scheda di integrazione dell'API HTTP: endpoint, schema delle risposte, scala di gravità, errori, limiti |
| [`aifa-archivio/esempi/`](aifa-archivio/esempi/) | risposte reali del server API, in JSON |
| `RCP_*.pdf`, `FI_*.pdf` | RCP e fogli illustrativi di 18 farmaci di uso comune, scaricati da AIFA |

Non sono nel repository, perché si rigenerano con una sincronizzazione: la cartella dati `archivio/`
(JSON e CSV dell'archivio, database DDInter, PDF scaricati) e l'open data `confezioni_fornitura.csv`.

## Avvio rapido

Serve [Rust](https://rustup.rs) (cargo). Su macOS Tauri usa WebKit di sistema, nessun'altra dipendenza.

```sh
cd aifa-archivio
cargo build --release
./target/release/aifa-gui ../archivio            # app desktop; alla prima apertura premere "Aggiorna ora"
./target/release/aifa-api --dir ../archivio      # server API su http://127.0.0.1:8787
./target/release/aifa-cli interazioni AUGMENTIN COUMADIN --dir ../archivio
```

La prima sincronizzazione scarica circa 100 MB (open data AIFA, liste A/H, DDInter, indice ATC) e dura 1–2 minuti.
I PDF vengono scaricati solo quando servono, oppure in blocco attivando l'opzione nell'app.

## Fonti dati

- AIFA open data `confezioni_fornitura.csv` (tutte le confezioni autorizzate, aggiornato ogni giorno)
- AIFA liste dei farmaci di classe A e H (prezzi, gruppi di equivalenza, lista di trasparenza)
- AIFA, RCP e FI in PDF da `api.aifa.gov.it`
- DDInter 2.0 (interazioni farmaco-farmaco, uso accademico non commerciale)
- Indice ATC/DDD dell'OMS (traduzione dei principi attivi in inglese)

## Avvertenza

Le interazioni sono un confronto testuale sul paragrafo 4.5 degli RCP e un database accademico:
non sostituiscono la valutazione clinica.
