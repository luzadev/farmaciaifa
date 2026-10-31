# aifa-archivio

Applicazione desktop in Rust (Tauri 2) per la gestione dei farmaci AIFA: archivio locale costruito dalle
liste ufficiali di classe A e H, ricerca, prezzi, Foglio Illustrativo (FI) e Riassunto delle Caratteristiche
del Prodotto (RCP) collegati a ogni confezione, aggiornamento periodico.

## Avvio

```sh
cargo build --release
./target/release/aifa-gui                      # archivio in ~/ArchivioAIFA
./target/release/aifa-gui /percorso/archivio   # archivio in una cartella a scelta
```

## Fonti dati

- **Open data AIFA** `https://drive.aifa.gov.it/farmaci/confezioni_fornitura.csv` (aggiornato ogni giorno, circa 82 MB):
  tutte le confezioni autorizzate (circa 160.000, 12.800 medicinali, incluse classe C, da banco, omeopatici e
  importazioni parallele) con titolare, stato amministrativo, procedura, forma, ATC, principio attivo, regime di
  fornitura e link diretti a FI e RCP. È la base dell'archivio.
- **Liste AIFA classe A e H** (pagina https://www.aifa.gov.it/liste-dei-farmaci, CSV "per nome commerciale"):
  agganciate per AIC, aggiungono classe, prezzo al pubblico, gruppo, gruppo di equivalenza e lista di trasparenza.
  Il programma legge la pagina per usare i link correnti; se non risponde usa gli ultimi noti (30-6-2026).
  Le poche confezioni presenti solo in lista (prodotti usciti dalla banca dati) restano senza PDF.

## Interfaccia

- **Archivio / Online**: ricerca istantanea nell'archivio locale oppure ricerca in tempo reale sull'API
  AIFA. Cerca per nome, principio attivo, azienda o AIC.
- **Dettagli**: prezzo al pubblico (se rimborsato), principio attivo, ATC, titolare, AIC, regime di fornitura,
  procedura, gruppo di equivalenza, lista di trasparenza, stato (sospesa) e importazione parallela. I pulsanti **Foglio illustrativo** e **RCP** scaricano il PDF (se manca o è più
  vecchio di 90 giorni) e lo aprono.
- **Barra archivio**: intervallo di aggiornamento in giorni (0 = manuale), opzione per scaricare in blocco
  tutti i PDF, "Aggiorna ora", log in tempo reale. All'avvio l'aggiornamento parte da solo se il precedente
  è più vecchio dell'intervallo.

## Interazioni tra farmaci

Scheda **Terapia**: si aggiungono i farmaci con "+ Terapia" dal pannello dei dettagli e si preme "Verifica interazioni".
Due fonti, mostrate separatamente:

1. **Paragrafo 4.5 degli RCP AIFA** (ufficiale, italiano). L'app scarica l'RCP di ogni farmaco, ne estrae il testo
   (`pdf-extract`, cache in `RCP.txt`), isola il paragrafo 4.5 e cerca per nome i principi attivi degli altri farmaci
   della terapia, mostrando il passaggio. È un confronto testuale: non riconosce le classi ("anticoagulanti",
   "inibitori del CYP3A4"). Il paragrafo 4.5 completo è consultabile anche dal dettaglio ("Interazioni (4.5)").
2. **DDInter** (ddinter.scbdd.com, database accademico, uso non commerciale): circa 220.000 coppie con gravità
   (Grave / Moderata / Lieve / Non classificata). Scaricato durante l'aggiornamento in `ddinter.csv`, con l'indice
   ATC dell'OMS (`atc.csv`) per tradurre i principi attivi italiani in nomi inglesi (via codice ATC, regole di
   suffisso e distanza di edit). I farmaci non mappati vengono elencati.

Nessuna delle due fonti sostituisce la valutazione clinica né una banca dati con licenza (Micromedex, Lexicomp, DrugBank).

Da riga di comando: `aifa-cli interazioni --dir /percorso/archivio 026089019 ZITROMAX COUMADIN` (AIC o nome).

## API HTTP (per altri programmi)

```sh
./target/release/aifa-api --dir /percorso/archivio            # http://127.0.0.1:8787
./target/release/aifa-api --dir /percorso/archivio --bind 0.0.0.0:8787   # raggiungibile dalla rete
```

Risposte JSON, CORS aperto, nessuna autenticazione (solo rete interna). I farmaci si indicano con AIC a 9 cifre, nome commerciale, AIC6, principio attivo o codice ATC. Da 2 a 20 farmaci per chiamata.

| Metodo | Percorso | Descrizione |
|---|---|---|
| GET | `/` | stato, versione, fonti e date di aggiornamento |
| GET | `/cerca?q=testo` | ricerca nell'archivio (nome, principio attivo, titolare, AIC), max 100 risultati |
| GET | `/farmaco/<AIC>` | anagrafica della confezione |
| GET | `/rcp/<AIC o nome>` | paragrafi 4.1–4.9 dell'RCP come testo |
| GET | `/interazioni?f=026089019&f=ZITROMAX&f=warfarin` | verifica interazioni |
| POST | `/interazioni` con body `{"farmaci": ["AUGMENTIN", "COUMADIN"]}` | come sopra |

Schema completo delle risposte, scala di gravità, codici di errore e tempi: **[API.md](API.md)**. Risposte reali del server in `esempi/`.

Esempio Python:

```python
import requests
r = requests.post("http://127.0.0.1:8787/interazioni", json={"farmaci": ["AUGMENTIN", "COUMADIN", "TACHIPIRINA"]}, timeout=60).json()
for i in r["ddinter"]["interazioni"]:
    print(i["gravita"], i["a"], "+", i["b"])
```

## File dell'archivio

- `farmaci.csv` (separatore `;`): aic, aic6, nome, confezione, titolare, stato, procedura, forma, atc, principio_attivo,
  fornitura, classe, prezzo_pubblico, gruppo, gruppo_equivalenza, lista_trasparenza, fi_pdf, rcp_pdf, fi_url, rcp_url.
- `archivio.json`: stessi dati, usati dall'app per la ricerca locale.
- `pdf/<aic6>_<NOME>/FI.pdf` e `RCP.pdf`: un documento per medicinale (copre tutte le confezioni).
  Per le importazioni parallele i link AIFA puntano ai PDF del prodotto originale.
- `state.json`: data ultimo aggiornamento, conteggi, liste usate, impostazioni.
- `ddinter.csv`, `atc.csv`: database interazioni e indice ATC (rinnovati ogni 30 giorni).

Tempi: aggiornamento senza PDF circa 1-2 minuti (download di 85 MB); con tutti i PDF circa 25.000 file
e diversi GB in più la prima volta, poi solo i mancanti o più vecchi di 90 giorni.

## Riga di comando

```sh
./target/release/aifa-cli sync  [--dir D] [--every GG] [--workers N] [--pdf-max-age GG] [--force] [--no-pdf]
./target/release/aifa-cli watch [stesse opzioni]   # resta attivo e risincronizza ogni --every giorni
```
Esempio cron: `0 3 * * * /percorso/aifa-cli sync --dir /percorso/archivio --every 7 >> /percorso/archivio/sync.log 2>&1`
