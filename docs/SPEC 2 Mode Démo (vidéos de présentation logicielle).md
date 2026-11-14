# Takebox — PRD : Mode Démo (vidéos de présentation logicielle)

Nov 7, 2026 · @François

## 0. Directives

Les spécifications suivantes font partie de la version payante de Takebox, nommée Takebox+
Ces fonctions ne sont donc pas accessibles dans la version gratuite.
L'activation de ces fonctions devra se faire par la délivrance d'une clé, pour l'instant hors scope de ce PRD.
Afin de simuler l'activation de la clé, un paramètre en settings nommé "Clé d'activation de Takebox+". La présence d'une chaine de caractère quelconque suffira à activer Takebox+, l'absence restreindra à Takebox version gratuite.

## 1. Résumé

Le **Mode Démo** transforme Takebox en outil de tournage pour les vidéos de présentation logicielle. L'utilisateur fait sa démo en parlant ; l'outil enregistre écran, captures, voix et événements sur une seule horloge, transcrit l'audio et produit un **Demo Package** : médias horodatés, transcription mot à mot, script découpé en scènes et timeline de montage. Claude reçoit ce package et le transforme en vidéo finie via des skills vidéo et un outil de montage.

Cette PRD complète la [spécification principale de Takebox](https://claude.ai/artifact/F5AaDJ4UdrVwmykNJC2hyK) : elle réutilise la barre, les raccourcis et le buffer (E2–E6) et ajoute les Epics E10 à E14.

### Problème

Une vidéo de démo de 3 minutes coûte aujourd'hui plusieurs heures : enregistrer, réécouter, écrire le script, retrouver le bon moment dans la vidéo, couper, synchroniser la voix, refaire les prises ratées. Les outils d'enregistrement produisent un fichier vidéo brut sans structure : rien ne dit *où* commence une scène ni *ce qui* y est dit.

### Objectifs

- Une session de démo produit, sans travail manuel, un package exploitable directement par Claude.
- Chaque média, chaque mot et chaque action est horodaté sur la même horloge (précision ≤ 50 ms).
- Le script est généré automatiquement à partir de la voix et découpé en scènes alignées sur les captures.
- Claude peut monter une première version de la vidéo à partir du package, sans réenregistrer.

### Hors périmètre (v1)

- Le montage lui-même dans Takebox (il est délégué à Claude et à l'outil de montage).
- Le doublage, la voix de synthèse et la traduction (pris en charge côté skills vidéo).
- L'hébergement ou la publication des vidéos.

## 2. Utilisateurs et parcours

| Persona | Besoin | Vidéo typique |
| --- | --- | --- |
| Product manager / product marketing | Annoncer une fonctionnalité vite et proprement | Teaser 60–90 s, release notes vidéo |
| Avant-vente / solution engineer | Démo client personnalisée, rejouable | Walkthrough 3–10 min |
| Formation  / support | Tutoriels pas à pas, mis à jour à chaque version | How-to 1–5 min |
| Développeur | Montrer une PR ou un bug à l'équipe | Clip brut 30 s–2 min |

### Parcours de bout en bout

1. **Préparer** : l'utilisateur clique sur *Session démo* dans la barre (ou ⌥⇧D). Il choisit le titre, la langue, la zone (écran, fenêtre, région), le micro, la webcam et, en option, un plan de scènes collé depuis un brief.
2. **Tourner** : il fait sa démo en parlant. Chaque clic, touche, changement de fenêtre est journalisé. Il pose un **marqueur de scène** (⌥⇧M) quand il change de sujet, prend des captures d'écran (⌥⇧S) pour les plans fixes, et marque une **prise ratée** (⌥⇧X) pour que le passage soit ignoré.
3. **Terminer** : ⌥⇧R arrête la session. La transcription locale démarre en arrière-plan ; la barre affiche la progression.
4. **Relire** : un panneau montre la timeline (scènes, captures, texte). L'utilisateur corrige un mot, renomme une scène, déplace un découpage.
5. **Envoyer à Claude** : *Exporter le package* crée un dossier `.cbdemo`, ou l'envoie directement à Claude (fichier joint, dossier connecté ou serveur MCP local). Claude propose un montage et un script final, l'utilisateur valide, l'outil de montage rend la vidéo.

## 3. Chaîne de bout en bout

&#91;embedded content: chaîne de bout en bout · du tournage à la vidéo finale\]

Tout ce qui est au-dessus du package reste sur le poste. Le package est la seule chose remise à Claude, qui suit ensuite l'une des deux voies : rendu automatique à partir de `edit.json`, ou montage humain à partir de `timeline.otio`.

## 4. Exigences fonctionnelles

Les identifiants DR-x (Demo Requirement) sont cités par les User Stories de la section 8. Les identifiants FR-x renvoient à la PRD principale.

### 4.1 Session de démo

- **DR-1** Une *session démo* démarre et s'arrête depuis la barre ou par ⌥⇧D / Ctrl+Maj+D. Elle peut être mise en pause (⌥⇧P) sans créer de nouveau fichier.
- **DR-2** Toute la session partage **une seule horloge monotone** (t0 = démarrage). Chaque élément porte `t_ms` depuis t0 et l'heure murale ISO 8601. Dérive entre pistes ≤ 50 ms sur 30 min.
- **DR-3** L'écran est enregistré en continu sur la zone choisie (FR-14). Les captures prises pendant la session (⌥⇧S) sont des plans fixes placés sur la même timeline.
- **DR-4** Marqueurs posés au clavier : **scène** (⌥⇧M, titre facultatif), **prise ratée** (⌥⇧X : du dernier marqueur à maintenant est à ignorer), **moment fort** (⌥⇧H : à mettre en valeur, zoom).
- **DR-5** Journal d'événements : clics (position, bouton), défilement, changements de fenêtre ou d'application active (titre), raccourcis clavier. La position du curseur est échantillonnée à 30 Hz pour permettre les zooms et le suivi automatique au montage.
- **DR-6** Par défaut, le **texte tapé n'est jamais enregistré** : seules les combinaisons de touches et la catégorie de la saisie (« saisie de texte, 14 caractères ») le sont.

### 4.2 Audio

- **DR-7** Micro et son système sont enregistrés sur deux pistes séparées (WAV 48 kHz), alignées sur l'horloge de session.
- **DR-8** Un vumètre sur la barre montre le niveau du micro ; un avertissement s'affiche si le micro est muet plus de 10 s pendant une session.
- **DR-9** Réduction de bruit facultative, appliquée à une copie : la piste brute est toujours conservée.

### 4.3 Transcription

- **DR-10** À la fin de la session, la piste micro est transcrite **localement**, avec un horodatage **par mot** et un score de confiance. Langues v1 : français et anglais, détection automatique.
- **DR-11** Les hésitations (« euh », « um »), les répétitions et les silences de plus de 1,5 s sont repérés et marqués comme coupes possibles, jamais supprimés.
- **DR-12** Un glossaire (noms de produits, termes métier) peut être fourni pour améliorer la reconnaissance.

### 4.4 Script

- **DR-13** L'outil produit un **script brut** découpé en scènes. Le découpage combine, par ordre de priorité : marqueurs de scène, changements de fenêtre, pauses longues de la voix.
- **DR-14** Chaque scène contient : titre, début et fin, texte de narration (transcription nettoyée), médias référencés (extraits vidéo, captures), résumé des actions à l'écran issu du journal (« clic sur *Exporter* », « ouvre *Paramètres* »).
- **DR-15** Si un plan de tournage a été importé (DR-1), chaque scène tournée est rattachée à la scène prévue ; les scènes manquantes sont signalées.
- **DR-16** Le script final rédigé est le travail de Claude (section 6) ; l'outil ne fait que structurer.

### 4.5 Relecture et export

- **DR-17** Un panneau de relecture affiche la timeline : pistes vidéo, captures, marqueurs, texte. L'utilisateur peut corriger un mot, renommer ou fusionner des scènes, déplacer une coupe. Chaque correction est conservée dans le package.
- **DR-18** *Exporter le package* écrit un dossier `.cbdemo` (zip facultatif) au format défini en section 5.
- **DR-19** Trois façons de remettre le package à Claude : fichier joint à une conversation ; écriture dans un dossier connecté à Claude ; serveur MCP local de Takebox qui expose les sessions (section 6).
- **DR-20** Masquage : des zones ou fenêtres à flouter (mots de passe, données client) se définissent avant le tournage et sont appliquées à tous les médias exportés. Les médias non floutés ne quittent jamais le poste.

### 4.6 Webcam

- **DR-21** La webcam est enregistrée sur sa **propre piste vidéo** (`webcam.mp4`, 720p ou 1080p, 30 i/s), sur l'horloge de session. Elle n'est jamais incrustée dans `screen.mp4` : la mise en page se décide au montage.
- **DR-22** Pendant le tournage, une bulle d'aperçu déplaçable montre le cadrage (reprend US-5.4 de la PRD principale). La bulle est exclue de l'enregistrement d'écran ; ses positions sont journalisées comme indication de mise en page.
- **DR-23** Le cadre du visage est détecté une fois par seconde et écrit dans `webcam.json`, pour recadrer automatiquement (carré, rond, 9:16) et savoir quand le présentateur est hors champ.
- **DR-24** Mises en page possibles au montage, choisies par Claude scène par scène dans `edit.json` : incrustation (coin au choix), côte à côte, plein écran (intro, conclusion), masquée. L'utilisateur peut fixer une mise en page par défaut.
- **DR-25** La webcam est désactivée par défaut et s'active par session. Le flou ou le remplacement d'arrière-plan s'applique au rendu, jamais à la piste brute. Les images webcam ne sont transmises à Claude qu'à la demande (`get_frame` avec la piste `webcam`).

## 5. Format du Demo Package

Le Demo Package est **le contrat** entre Takebox et Claude : un dossier `.cbdemo` versionné (`schema_version`), lisible par un humain et par une machine, sans format propriétaire. Tous les temps sont en millisecondes depuis t0.

```
onboarding-v2.cbdemo/
  manifest.json          métadonnées, pistes, version du schéma
  script.json            scènes structurées (DR-13, DR-14)
  script.md              même contenu, lisible
  transcript.json        mots horodatés + confiance (DR-10)
  transcript.srt         sous-titres prêts à l'emploi
  events.jsonl           journal d'événements, une ligne par événement (DR-5)
  markers.json           scènes, prises ratées, moments forts (DR-4)
  webcam.json            cadre du visage, positions de la bulle (DR-22, DR-23)
  timeline.otio          timeline OpenTimelineIO pour les outils de montage
  media/
    screen.mp4           enregistrement continu (H.264, image clé chaque seconde)
    webcam.mp4           piste webcam séparée (DR-21)
    stills/0007.png      captures fixes
    thumbs/              une vignette par scène
  audio/
    mic.wav              piste micro brute
    mic.clean.wav        piste nettoyée (si DR-9)
    system.wav           son système
```

### manifest.json

```json
{
  "schema_version": "1.0",
  "title": "Onboarding v2",
  "language": "fr",
  "started_at": "2026-11-07T10:42:03.120+01:00",
  "duration_ms": 412870,
  "capture": { "mode": "window", "app": "Acme Studio", "size": [2560, 1440], "scale": 2.0, "fps": 30 },
  "tracks": [
    { "id": "screen", "kind": "video", "path": "media/screen.mp4", "offset_ms": 0 },
    { "id": "mic", "kind": "audio", "path": "audio/mic.wav", "offset_ms": 12 },
    { "id": "system", "kind": "audio", "path": "audio/system.wav", "offset_ms": 0 },
    { "id": "webcam", "kind": "video", "path": "media/webcam.mp4", "offset_ms": 33, "size": [1280, 720] }
  ],
  "redactions": [{ "rect": [1840, 120, 520, 64], "from_ms": 0, "to_ms": 412870 }],
  "generator": { "app": "Takebox", "version": "1.2.0", "asr_model": "[MODÈLE]" }
}
```

### Une scène dans script.json

```json
{
  "id": "s03",
  "title": "Créer un projet",
  "start_ms": 48210,
  "end_ms": 91540,
  "source": "marker",
  "narration": "On clique sur Nouveau projet, on choisit le modèle Analyse…",
  "words": [48390, 91120],
  "actions": [
    { "t_ms": 52004, "type": "click", "target": "Nouveau projet", "pos": [312, 96] },
    { "t_ms": 60870, "type": "window", "title": "Acme Studio — Modèles" }
  ],
  "media": [
    { "track": "screen", "in_ms": 48210, "out_ms": 91540 },
    { "track": "webcam", "in_ms": 48210, "out_ms": 91540 },
    { "still": "media/stills/0007.png", "t_ms": 74300 }
  ],
  "cuts": [{ "from_ms": 66100, "to_ms": 68900, "reason": "silence" }],
  "highlights": [{ "t_ms": 80250, "pos": [1210, 640] }],
  "planned_scene": "P2",
  "edited_by_user": false
}
```

| Fichier | Lu par | Usage principal |
| --- | --- | --- |
| `script.json` / `script.md` | Claude | Rédiger le script final, choisir les plans |
| `transcript.json` | Claude, outil de montage | Coupes au mot près, sous-titres |
| `events.jsonl` | Claude | Décrire les actions, placer zooms et annotations |
| `timeline.otio` | Outil de montage | Première timeline prête à monter |
| `media/`, `audio/` | Outil de montage, Claude (images extraites) | Rendu final |
| webcam.mp4, webcam.json | Claude, moteur de rendu | Placer et recadrer le présentateur scène par scène |

## 6. Intégration Claude

Takebox s'arrête au package : il **tourne et structure**. Claude **écrit et décide** (script final, plan de montage). L'outil de montage **rend** la vidéo. Chaque étape lit un fichier et en écrit un autre, ce qui permet de relancer n'importe quelle étape sans tout refaire.

### Ce que Claude fait du package

1. **Lire** `manifest.json`, puis `script.json` ; ouvrir `transcript.json` et `events.jsonl` seulement pour les scènes à travailler ; extraire quelques images clés pour voir l'écran.
2. **Rédiger le script final** selon un brief : public, ton, durée cible (ex. 90 s), langue. Les passages ratés (DR-4) et les hésitations (DR-11) sont écartés.
3. **Produire le plan de montage** `edit.json` : liste ordonnée de plans (source, entrée, sortie), zooms sur les moments forts, mise en page de la webcam (incrustation, côte à côte, plein écran ou masquée), sous-titres, titres de scène, transitions, et choix de la voix : voix d'origine nettoyée, réenregistrement guidé par le script, ou voix de synthèse via un skill.
4. **Faire rendre** la vidéo par l'outil de montage, regarder le résultat (images extraites) et corriger le plan si besoin.
5. **Décliner** : version courte, autres langues, formats 16:9 / 9:16 / 1:1, à partir du même package.

### Skills vidéo

Les skills vidéo déjà disponibles sur le compte (ex. *video-making*, *axway-motion-video*) prennent le package en entrée. Un skill dédié **demo-to-video** est à écrire : il décrit comment lire un `.cbdemo`, les règles de rédaction d'un script de démo, et le schéma `edit.json`. Il s'appuie sur les skills existants pour la charte graphique, la musique et la voix.

### Outil de montage

Deux voies, toutes deux alimentées par le même package :

| Voie | Comment | Pour qui |
| --- | --- | --- |
| Rendu automatique | Un moteur de rendu piloté par `edit.json` (basé sur FFmpeg ou un moteur de composition vidéo), appelé par Claude | Vidéos récurrentes, déclinaisons, zéro montage manuel |
| Montage humain | `timeline.otio` importé dans un logiciel de montage compatible OpenTimelineIO (DaVinci Resolve, Premiere via adaptateur, etc.) | Vidéos marketing soignées, retouches fines |

Le choix du moteur de rendu automatique est une question ouverte (section 9).

### Serveur MCP local

Takebox expose ses sessions à Claude par un serveur MCP local (DR-19), sans copier les fichiers :

| Outil MCP | Rôle |
| --- | --- |
| `list_demos` | Sessions disponibles : titre, date, durée, nombre de scènes |
| `get_demo(id)` | `manifest.json` + `script.json` |
| `get_transcript(id, from_ms, to_ms)` | Mots horodatés d'un intervalle |
| `get_events(id, from_ms, to_ms)` | Actions à l'écran d'un intervalle |
| `get_frame(id, t_ms, track)` | Image PNG de l'écran ou de la webcam à un instant donné (floutée selon DR-20) |
| `export_clip(id, in_ms, out_ms)` | Extrait vidéo prêt pour le rendu |
| `update_scene(id, scene)` | Écrit une correction de Claude dans le package (désactivé par défaut) |

Le serveur n'écoute que sur la machine locale et est en lecture seule par défaut.

## 7. Exigences non fonctionnelles et choix techniques

Les chiffres sont des cibles v1, à confirmer par des tests. Les NFR de la PRD principale s'appliquent aussi (exécutable autonome, licences permissives, hors ligne).

| ID | Domaine | Exigence |
| --- | --- | --- |
| DNFR-1 | Synchronisation | Dérive ≤ 50 ms entre écran, webcam, audio et événements sur 30 min |
| DNFR-2 | Transcription | Durée de transcription ≤ 50 % de la durée de session sur un portable 4 cœurs de 2020, sans GPU |
| DNFR-3 | Qualité | Taux d'erreur par mot ≤ 10 % en français et en anglais, sur micro casque, avec glossaire |
| DNFR-4 | Disponibilité | Le package (sans transcription) est exportable ≤ 5 s après l'arrêt |
| DNFR-5 | Taille | ≤ 500 Mo par tranche de 10 min en 1440p30, + 150 Mo avec la webcam en 720p |
| DNFR-6 | Robustesse | Un plantage pendant la session laisse un package récupérable jusqu'à la dernière seconde enregistrée |
| DNFR-7 | Confidentialité | Tout reste local. Rien n'est envoyé à Claude sans action explicite. Le texte tapé n'est jamais journalisé (DR-6). Les masquages (DR-20) sont appliqués avant tout export. |
| DNFR-8 | Compatibilité | Le schéma du package est versionné ; une version mineure ne casse jamais un lecteur existant |

### Choix techniques

| Besoin | Choix proposé | Remarque |
| --- | --- | --- |
| Transcription locale | whisper.cpp (licence MIT) via des bindings Rust | Accélération Metal sur Mac, CPU partout ailleurs, GPU facultatif |
| Modèle de transcription | Téléchargé au premier usage du Mode Démo, puis gardé en cache local | Garde l'exécutable léger ; une « édition hors ligne » peut l'embarquer (question ouverte) |
| Journal d'événements | macOS : event tap (permission Accessibilité) ; Windows : hooks souris et clavier bas niveau ; Linux X11 : XInput2 | Wayland : voir les risques (section 9) |
| Horloge commune | Horloge monotone de l'OS, horodatage à la source de chaque piste | Recalage audio/vidéo à l'export |
| Timeline de montage | Fichier OpenTimelineIO (JSON) écrit directement | Pas de dépendance C++ à embarquer |
| Vidéo | H.264 matériel (PRD principale, E5), image clé chaque seconde | Coupes précises sans réencodage complet |
| Webcam et cadrage | Caméra : AVFoundation (macOS), Media Foundation (Windows), PipeWire / V4L2 (Linux). Visage : Vision (macOS), Windows.Media.FaceAnalysis, modèle léger à licence permissive sous Linux | Second flux encodé en matériel ; capture caméra partagée avec US-5.4 |

## 8. Backlog — Epics, User Stories, Tâches

Le Mode Démo ajoute 5 Epics et 22 User Stories, numérotés à la suite de la PRD principale. Il dépend de E5 (enregistrement) et E6 (buffer). Les écrans du Mode Démo (configuration, relecture) n'ont pas encore de maquette : elles sont à ajouter au canevas sous les références M4 et M5.

| Epic | Titre | Couvre | Dépend de |
| --- | --- | --- | --- |
| E10 | Session démo et horodatage | DR-1–6, DR-20–25 | E5 |
| E11 | Audio et transcription | DR-7–12 | E10 |
| E12 | Script et relecture | DR-13–17 | E11 |
| E13 | Demo Package et remise à Claude | DR-18, DR-19 | E12 |
| E14 | Montage par Claude | Section 6 | E13 |

### E10 · Session démo et horodatage

**US-10.1** En tant que présentateur, je veux lancer une session démo en un raccourci, pour enregistrer écran, voix et captures ensemble. *(M4 à créer · DR-1, DR-3)*

- CA : ⌥⇧D ouvre la configuration (titre, langue, zone, micro) ; Entrée démarre ; la barre passe en mode enregistrement avec un badge « Démo ».

* [x] Écran de configuration de session
* [x] Orchestrateur de session au-dessus de l'enregistreur E5
* [x] Captures fixes rangées dans la session et dans le buffer

**US-10.2** En tant que monteur, je veux que tous les éléments partagent la même horloge, pour couper au mot près. *(DR-2, DNFR-1)*

- CA : sur une session de 30 min avec un clap test (bip + flash), décalage mesuré ≤ 50 ms entre audio, vidéo et événements.

* [ ] Service d'horloge de session (monotone)
* [ ] Horodatage à la source pour chaque piste
* [ ] Outil de test « clap » et rapport de dérive

**US-10.3** En tant que présentateur, je veux poser des marqueurs de scène, de prise ratée et de moment fort sans m'arrêter. *(DR-4)*

- CA : ⌥⇧M, ⌥⇧X, ⌥⇧H posent un marqueur visible un court instant sur la barre ; ils apparaissent dans `markers.json`.

* [ ] Raccourcis et retour visuel
* [ ] Modèle de marqueurs

**US-10.4** En tant que rédacteur du script, je veux savoir ce qui s'est passé à l'écran (clics, fenêtres), sans que le texte tapé soit enregistré. *(DR-5, DR-6)*

- CA : `events.jsonl` contient clics, défilements, changements de fenêtre et positions curseur à 30 Hz ; aucune frappe de texte en clair.

* [ ] Capture d'événements par OS (event tap, hooks Windows, XInput2)
* [ ] Filtre de confidentialité des frappes
* [ ] Demande de permission Accessibilité sur macOS

**US-10.5** En tant que présentateur, je veux masquer des zones ou des fenêtres avant de tourner. *(DR-20, DNFR-7)*

- CA : une zone définie est floutée dans `screen.mp4`, les captures, les vignettes et `get_frame`.

* [ ] Outil de sélection de zones et de fenêtres
* [ ] Flou appliqué à l'export

**US-10.6** En tant que présentateur, je veux apparaître dans ma démo via la webcam, sur une piste séparée. *(DR-21–23, DR-25)*

- CA : la webcam s'active dans la configuration de session ; `webcam.mp4` est synchronisée à ≤ 50 ms ; la bulle n'apparaît pas dans `screen.mp4` ; `webcam.json` contient un cadre de visage par seconde.

* [ ] Piste webcam dans l'orchestrateur de session (réutilise la capture caméra de US-5.4)
* [ ] Bulle d'aperçu exclue de l'enregistrement d'écran
* [ ] Détection de visage par OS et écriture de `webcam.json`

### E11 · Audio et transcription

**US-11.1** En tant que présentateur, je veux ma voix et le son du logiciel sur deux pistes, avec un vumètre. *(DR-7, DR-8)*

- CA : `mic.wav` et `system.wav` en 48 kHz ; alerte après 10 s de micro muet.

* [ ] Écriture WAV par piste
* [ ] Vumètre et alerte micro muet

**US-11.2** En tant que présentateur, je veux une piste voix nettoyée sans perdre l'originale. *(DR-9)*

- CA : `mic.clean.wav` produit en arrière-plan ; `mic.wav` intact.

* [ ] Réduction de bruit (bibliothèque à licence permissive)

**US-11.3** En tant que présentateur, je veux une transcription locale avec un temps par mot. *(DR-10, DR-12, DNFR-2, DNFR-3)*

- CA : FR et EN détectés ; chaque mot porte début, fin et confiance ; le glossaire corrige les noms de produits ; cibles DNFR-2/3 atteintes sur le jeu de test.

* [ ] Intégration whisper.cpp
* [ ] Téléchargement du modèle et cache
* [ ] Glossaire en invite initiale
* [ ] Jeu de test FR/EN de 30 min et mesure du taux d'erreur

**US-11.4** En tant que monteur, je veux que les hésitations et silences soient repérés. *(DR-11)*

- CA : « euh », « um », répétitions et silences > 1,5 s apparaissent comme coupes proposées dans `script.json`.

* [ ] Détecteur d'hésitations FR/EN
* [ ] Détection de silences sur la piste

### E12 · Script et relecture

**US-12.1** En tant que présentateur, je veux un script brut découpé en scènes, avec actions et médias. *(DR-13, DR-14)*

- CA : chaque scène a titre, bornes, narration, actions, médias ; `script.md` est lisible sans outil.

* [ ] Algorithme de découpage (marqueurs › fenêtres › pauses)
* [ ] Résumé des actions depuis le journal
* [ ] Génération `script.md`

**US-12.2** En tant que présentateur, je veux importer un plan de tournage et voir ce qui manque. *(DR-15)*

- CA : un plan collé (liste de scènes) est rattaché aux scènes tournées ; les scènes manquantes sont listées à l'arrêt.

* [ ] Import du plan
* [ ] Rapprochement scènes prévues / tournées

**US-12.3** En tant que présentateur, je veux relire et corriger la timeline avant d'envoyer à Claude. *(M5 à créer · DR-17)*

- CA : corriger un mot, renommer, fusionner ou scinder une scène, déplacer une coupe ; lecture synchronisée vidéo + texte ; les corrections sont marquées `edited_by_user`.

* [ ] Panneau timeline (pistes, marqueurs, texte)
* [ ] Lecteur vidéo synchronisé au texte
* [ ] Persistance des corrections

### E13 · Demo Package et remise à Claude

**US-13.1** En tant que présentateur, je veux exporter un package `.cbdemo` complet. *(DR-18, DNFR-4, DNFR-5)*

- CA : le dossier respecte la section 5 ; zip facultatif ; export ≤ 5 s hors transcription.

* [ ] Écriture du package et du manifest
* [ ] Option zip

**US-13.2** En tant que monteur, je veux ouvrir la session dans mon logiciel de montage. *(Section 6)*

- CA : `timeline.otio` s'ouvre dans au moins un logiciel compatible OpenTimelineIO, scènes et marqueurs visibles.

* [ ] Écriture OpenTimelineIO (pistes, clips, marqueurs)
* [ ] Test d'import dans un logiciel de montage

**US-13.3** En tant qu'utilisateur de Claude, je veux que Claude lise mes sessions sans que je copie de fichiers. *(DR-19)*

- CA : les 7 outils MCP de la section 6 répondent ; serveur local uniquement ; `update_scene` désactivé par défaut.

* [ ] Serveur MCP local intégré à l'exécutable
* [ ] Extraction d'image et d'extrait à la demande

**US-13.4** En tant que développeur de skills, je veux un schéma publié et un validateur. *(DNFR-8)*

- CA : schémas JSON publiés dans le dépôt ; `takebox validate <dossier>` signale toute erreur.

* [ ] Schémas JSON versionnés
* [ ] Commande de validation

### E14 · Montage par Claude

**US-14.1** En tant qu'utilisateur de Claude, je veux un skill **demo-to-video** qui transforme un package en script final et plan de montage. *(Section 6)*

- CA : à partir d'un brief (public, ton, durée), Claude produit `script.final.md` et `edit.json` valides, sans les prises ratées.

* [ ] Rédaction du skill (lecture du package, règles d'écriture, schéma `edit.json`)
* [ ] Trois packages d'exemple pour tester le skill

**US-14.2** En tant qu'utilisateur, je veux que la vidéo soit rendue automatiquement à partir de `edit.json`.

- CA : MP4 1080p + SRT + vignette rendus sans intervention ; zooms sur les moments forts ; sous-titres synchronisés.

* [ ] Choix du moteur de rendu (question ouverte)
* [ ] Schéma `edit.json` v1
* [ ] Rendu des zooms, titres et sous-titres

**US-14.3** En tant qu'utilisateur, je veux que Claude vérifie son propre montage.

- CA : après le rendu, Claude regarde des images extraites, signale les plans coupés au milieu d'une action et corrige `edit.json`.

* [ ] Boucle rendu → extraction d'images → correction

**US-14.4** En tant que product marketing, je veux décliner une démo en plusieurs versions.

- CA : depuis un package : version courte (≤ 60 s), version complète, formats 16:9 / 9:16 / 1:1, sous-titres FR et EN.

* [ ] Recadrage intelligent guidé par la position du curseur
* [ ] Traduction des sous-titres via skill

**US-14.5** En tant qu'utilisateur, je veux que Claude choisisse où placer la webcam dans chaque scène. *(DR-24)*

- CA : `edit.json` porte une mise en page webcam par scène : plein écran pour l'intro et la conclusion, incrustation pendant les manipulations, masquée sur les plans de détail ; le moteur de rendu applique recadrage et flou d'arrière-plan.

* [ ] Champ `webcam_layout` dans le schéma `edit.json`
* [ ] Rendu des quatre mises en page
* [ ] Règles de choix dans le skill demo-to-video

## 9. Mesures de succès, risques, questions ouvertes

### Mesures de succès

Ces cibles sont à confirmer avec les premiers utilisateurs.

| Mesure | Cible v1 |
| --- | --- |
| Temps entre fin du tournage et première vidéo montée par Claude, pour une démo de 5 min | ≤ 15 min |
| Part des scènes du script brut acceptées sans correction manuelle | ≥ 80 % |
| Part des sessions où aucune piste n'est désynchronisée | ≥ 99 % |
| Vidéos publiées sans retour au logiciel de montage | ≥ 50 % |

### Risques

| Risque | Impact | Parade |
| --- | --- | --- |
| Wayland interdit l'écoute globale des clics et du clavier | Pas de journal d'événements sur GNOME/KDE Wayland | v1 sans `events.jsonl` sur Wayland (le reste fonctionne) ; suivre l'API d'émulation d'entrées (libei) et les portails |
| La permission Accessibilité de macOS effraie les utilisateurs | Abandon à l'installation | Demander la permission seulement au premier lancement du Mode Démo, avec l'explication de DR-6 |
| La transcription est lente sur les vieux PC | Attente après le tournage | Modèle plus petit proposé automatiquement ; transcription par blocs pendant le tournage |
| Des données sensibles apparaissent à l'écran | Fuite dans la vidéo ou vers Claude | Masquage avant tournage (DR-20) ; aperçu des images envoyées ; aucune remise automatique |
| Le format du package évolue plus vite que les skills | Skills cassés | Schéma versionné et validateur (US-13.4) |
| Taille des packages (vidéo brute) | Lenteur d'envoi à Claude | Serveur MCP local (US-13.3) : Claude lit des images et extraits à la demande, sans téléverser la vidéo entière |
| Deux flux vidéo (écran + webcam) à encoder en même temps | Images perdues sur les PC modestes | Encodeur matériel pour les deux flux ; webcam ramenée à 720p si la charge dépasse le seuil |

### Questions ouvertes

- [ ] Quel moteur de rendu automatique pour US-14.2 : FFmpeg piloté par `edit.json`, ou un moteur de composition (vidéo décrite en HTML ou en code) déjà utilisé par les skills vidéo ?
- [ ] Le modèle de transcription est-il embarqué (exécutable plus lourd, 100 % hors ligne) ou téléchargé au premier usage ?
- [ ] Faut-il transcrire aussi le son système (vidéos ou appels montrés pendant la démo) ?
- [ ] Le Mode Démo fait-il partie de la v1.0, ou d'une v1.2 après l'annotation (E7) ?
- [ ] Quelle charte graphique par défaut pour les vidéos rendues (titres, sous-titres, musique) ?
