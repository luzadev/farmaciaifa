# API aifa-archivio — scheda di integrazione

Server HTTP JSON locale (`aifa-api`, binario Rust, nessuna dipendenza esterna a runtime).
Base URL di default: `http://127.0.0.1:8787` (si cambia con `--bind 0.0.0.0:8787`).
Versione: 0.3.0. Risposte sempre `application/json; charset=utf-8`, CORS `*`.
Esempi reali completi (output non modificato del server) nella cartella `esempi/`.

## 1. Endpoint

| Metodo e percorso | Scopo | Esempio |
|---|---|---|
| `GET /` | stato del servizio, versione, fonti e date di aggiornamento | `esempi/GET_root.json` |
| `GET /cerca?q=testo` | ricerca libera nell'archivio (nome, principio attivo, titolare, AIC), max 100 risultati | |
| `GET /farmaco/<AIC>` | anagrafica di una confezione (9 cifre) | `esempi/GET_farmaco.json` |
| `GET /rcp/<riferimento>` | paragrafi 4.1–4.9 dell'RCP come testo | `esempi/GET_rcp_COUMADIN.json` |
| `GET /interazioni?f=…&f=…` | verifica interazioni fra 2–20 farmaci | `esempi/GET_interazioni_AUGMENTIN_COUMADIN.json` |
| `POST /interazioni` body `{"farmaci":["…","…"]}` | come sopra, via JSON | `esempi/POST_interazioni.json` |

## 2. Identificazione del farmaco (`f=` e `/rcp/`)

Ogni riferimento viene risolto in quest'ordine, al primo esito:

1. AIC a 9 cifre (`026089019`) → confezione esatta.
2. Nome commerciale esatto, senza distinzione maiuscole (`COUMADIN`).
3. AIC6 numerico (`26089`).
4. Principio attivo esatto come scritto da AIFA (`WARFARIN SODICO`, `AMOXICILLINA/ACIDO CLAVULANICO`).
5. Codice ATC esatto (`B01AA03`).
6. Prefisso del nome commerciale, almeno 4 caratteri (`AUGM`).
7. Principio attivo senza sale, almeno 4 caratteri (`warfarin` → WARFARIN SODICO, `paracetamolo`).

Per i punti 2–7 viene scelta una confezione con RCP in banca dati, preferendo quelle rimborsate (classe A/H).
Un principio attivo o un ATC vengono quindi risolti su **un** medicinale rappresentativo (es. `J01FA10` → AZACID): va bene per le interazioni, che dipendono dal principio, ma il campo `nome` della risposta riporterà quel medicinale. Due riferimenti che portano allo stesso medicinale vengono conteggiati una volta sola.

Il campo `farmaci[].richiesto` restituisce sempre la stringa inviata, così il chiamante può riallineare.

## 3. Risposta di `/interazioni`

```jsonc
{
  "versione": "0.3.0",
  "fonti": { … },                 // vedi §5, da copiare nell'audit
  "farmaci": [                    // riferimenti riconosciuti
    { "richiesto": "AUGMENTIN", "aic": "026089019", "nome": "AUGMENTIN",
      "principio_attivo": "AMOXICILLINA/ACIDO CLAVULANICO", "atc": "J01CR02", "titolare": "GLAXOSMITHKLINE S.P.A." }
  ],
  "non_trovati": [],              // riferimenti non riconosciuti (stringhe inviate)
  "ddinter": {
    "disponibile": true,          // false se il database DDInter non è stato scaricato
    "non_mappati": [],            // farmaci il cui principio non è stato tradotto in un nome DDInter: per loro il controllo DDInter NON è avvenuto
    "interazioni": [
      { "a": "AUGMENTIN", "b": "COUMADIN",            // nomi commerciali come risolti
        "nome_a": "amoxicillin", "nome_b": "warfarin", // nomi DDInter (inglese)
        "livello": "Moderate",                         // valore originale DDInter
        "gravita": "moderata" }                        // valore italiano, vedi §4
    ]
  },
  "rcp": {
    "interazioni": [
      { "a": "AUGMENTIN",                 // RCP in cui è stata trovata la citazione
        "b": "COUMADIN",                  // farmaco citato
        "componente": "warfarin",         // componente del principio attivo di b trovato nel testo
        "segnale": "citazione",           // "controindicazione" | "evitare" | "citazione", vedi §4
        "testo": "…brano del paragrafo 4.5 di a, ±200 caratteri attorno alla citazione…" }
    ],
    "paragrafi_4_5": [                    // il paragrafo 4.5 completo di ogni farmaco
      { "farmaco": "AUGMENTIN", "testo": "4.5 Interazioni con altri medicinali …", "errore": null,
        "nomi_ddinter": ["amoxicillin", "clavulanic acid"] }
    ]
  },
  "scala_gravita": ["grave", "moderata", "lieve", "non classificata"],
  "avvertenza": "…"
}
```

Campi **non disponibili** (non esistono nelle fonti usate): meccanismo, raccomandazione strutturata, sezione RCP diversa dalla 4.5. Il "testo" dell'RCP è l'unica descrizione; DDInter fornisce solo la coppia e il livello.

Le coppie RCP sono direzionali: se l'RCP di A cita B **e** l'RCP di B cita A compaiono due voci. Le coppie DDInter sono una per coppia, ordinate per gravità decrescente.

### Nessuna interazione
HTTP 200 con `ddinter.interazioni: []` e `rcp.interazioni: []` (`esempi/GET_interazioni_nessuna.json`). Controllare anche `ddinter.non_mappati` e `rcp.paragrafi_4_5[].errore`: se valorizzati, l'assenza di risultati per quel farmaco è "non verificato", non "nessuna interazione".

### Farmaco non riconosciuto
- Se restano almeno 2 farmaci riconosciuti: HTTP 200, il riferimento finisce in `non_trovati` e il resto viene verificato (`esempi/GET_interazioni_uno_non_riconosciuto.json`).
- Se ne restano meno di 2: HTTP 404, `codice: "farmaci_non_trovati"`, con `farmaci` e `non_trovati` (`esempi/GET_interazioni_404.json`).

## 4. Scala di gravità

**DDInter** (`ddinter.interazioni[].gravita`), valori esatti:

| `livello` (DDInter) | `gravita` | proposta di mappatura nel selettore |
|---|---|---|
| `Major` | `grave` | esclusione dalla lista |
| `Moderate` | `moderata` | evidenza al medico |
| `Minor` | `lieve` | nota |
| `Unknown` | `non classificata` | nota |

DDInter non ha un livello "controindicata": non viene mai emesso.

**RCP** (`rcp.interazioni[].segnale`), classificazione **lessicale** del brano, non clinica:

| `segnale` | criterio | proposta |
|---|---|---|
| `controindicazione` | il brano contiene "controindicat…" | esclusione |
| `evitare` | contiene "evitare", "sconsigliat…", "non deve/devono essere", "non raccomandat…" | evidenza al medico |
| `citazione` | il principio è nominato nel 4.5 senza quei termini | nota, mostrare `testo` |

Il `testo` va sempre mostrato al medico: una citazione in un elenco ("…amoxicillina, benzilpenicillina…") pesa meno di una frase dedicata, e l'API non le distingue.

## 5. Fonti e data di aggiornamento

`fonti` è presente in `GET /` e in ogni risposta di `/interazioni`:

```json
"fonti": {
  "archivio": { "fonte": "AIFA open data confezioni_fornitura.csv + liste classe A/H", "aggiornato": "2026-10-29" },
  "rcp":      { "fonte": "AIFA, Riassunto delle Caratteristiche del Prodotto, paragrafo 4.5 (PDF scaricato da api.aifa.gov.it, rinnovato ogni 90 giorni)" },
  "ddinter":  { "fonte": "DDInter 2.0 (ddinter.scbdd.com), uso accademico non commerciale", "aggiornato": "2026-10-29" }
}
```

- RCP: PDF ufficiale AIFA del medicinale, scaricato alla prima richiesta e rinnovato se più vecchio di 90 giorni. Il testo è estratto dal PDF, quindi tabelle ed elenchi perdono l'impaginazione.
- DDInter 2.0: database accademico (Xiong et al., Nucleic Acids Research 2022), licenza per uso non commerciale. Verificare la licenza prima di un uso in prodotto.
- Nessun uso di Farmadati o Codifa.

## 6. `GET /farmaco/<AIC>` e `GET /rcp/<riferimento>`

`/farmaco` restituisce **solo l'anagrafica** della confezione: `aic`, `aic6`, `nome`, `denominazione` (descrizione confezione con dosaggio), `forma`, `atc`, `principio`, `titolare`, `stato`, `procedura`, `fornitura` (regime), `classe` (A/H, vuota se non rimborsata), `prezzo`, `gruppo`, `gruppo_eq`, `trasparenza`, `sis`/`pdf_sis`/`pdf_aic6` (chiavi interne AIFA per i PDF). Non c'è un campo "dosaggio" separato: è dentro `denominazione`.

`/rcp` restituisce i paragrafi dell'RCP come testo, chiavi `"4.1"` … `"4.9"` (quelle trovate):

| chiave | contenuto |
|---|---|
| 4.1 | indicazioni terapeutiche |
| 4.2 | posologia e modo di somministrazione (comprende la posologia pediatrica e gli aggiustamenti renali/epatici) |
| 4.3 | controindicazioni |
| 4.4 | avvertenze speciali e precauzioni |
| 4.5 | interazioni |
| 4.6 | fertilità, gravidanza e allattamento |
| 4.7 | capacità di guidare |
| 4.8 | effetti indesiderati |
| 4.9 | sovradosaggio |

Più `aic`, `nome`, `principio_attivo`, `atc`, `fonte`, `rcp_url` (PDF originale). Il livello 4 può leggere da qui gravidanza, età e rene, ma come testo libero: non esistono campi strutturati.
Se il medicinale ha più forme nello stesso RCP (es. compresse e sospensione), viene restituita la prima serie di paragrafi.

## 7. Limiti e prestazioni

| | |
|---|---|
| farmaci per chiamata | 2–20 (oltre: HTTP 400 `troppi_farmaci`) |
| prima chiamata per un farmaco | 1–3 s per farmaco (scarica il PDF da AIFA ed estrae il testo) |
| chiamate successive | 20–50 ms (testo in cache su disco, nessun accesso a rete) |
| concorrenza | un thread per richiesta |
| autenticazione | nessuna: va esposto solo su rete interna o dietro un reverse proxy con token |
| base URL di test | lo stesso binario con `--dir` su una copia dell'archivio, es. `aifa-api --dir /percorso/archivio --bind 127.0.0.1:8788` |

Una terapia cronica di 10–12 farmaci costa quindi fino a ~30 s alla prima verifica e meno di 0,1 s dopo. Se serve, si può preriscaldare la cache chiamando `/rcp/<AIC>` per i farmaci più usati.

## 8. Codici di errore

| HTTP | `codice` | quando |
|---|---|---|
| 400 | `pochi_farmaci` | meno di 2 riferimenti inviati |
| 400 | `troppi_farmaci` | più di 20 riferimenti |
| 400 | — | body POST non è JSON valido |
| 404 | `farmaci_non_trovati` | meno di 2 riferimenti riconosciuti (`non_trovati` elenca quali) |
| 404 | — | `/farmaco` AIC inesistente, `/rcp` farmaco inesistente o senza RCP, endpoint sconosciuto |
| 502 | — | `/rcp`: PDF non scaricabile da AIFA o testo non estraibile |
| 503 | — | archivio non caricato (mai sincronizzato) |

Tutte le risposte di errore hanno `{"errore": "testo leggibile", …}`.
Servizio non raggiungibile (connessione rifiutata, timeout): nessuna risposta, il chiamante deve trattarlo come "verifica non eseguita" e ricadere sulle liste interne, segnalandolo al medico. Consiglio: timeout client di 60 s per `/interazioni` (prima chiamata) e `GET /` come health check.

## 9. Esempio minimo (Python)

```python
import requests
r = requests.post("http://127.0.0.1:8787/interazioni", json={"farmaci": ["AUGMENTIN", "COUMADIN", "warfarin"]}, timeout=60)
r.raise_for_status()
d = r.json()
for i in d["ddinter"]["interazioni"]:
    print("DDInter", i["nome_a"], i["nome_b"], i["gravita"])
for i in d["rcp"]["interazioni"]:
    print("RCP", i["a"], "cita", i["componente"], i["segnale"])
print("fonti:", d["fonti"])
```
