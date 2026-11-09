# Capture Bar — français. Mêmes messages que en.ftl (un test le vérifie).
# Ponctuation : espace insécable avant « ? », « : » et « ; ».

## Barre (M2.1, M2.2, M2.3)

grip-label = Déplacer la barre
grip-tip = Glisser pour déplacer. Double-cliquer pour ancrer
pill-expand = Déplier la barre
bar-capture = Capturer
bar-capture-tip = Capturer avec le mode actuel ({ $key })
bar-repeat-tip = Capturer à nouveau la même zone ({ $key })
bar-stop = Arrêter
bar-stop-tip = Arrêter l'enregistrement ({ $key })
bar-pause = Pause
bar-pause-tip = Mettre l'enregistrement en pause ({ $key })
bar-resume = Reprendre
bar-resume-tip = Reprendre l'enregistrement ({ $key })
bar-discard = Jeter
bar-discard-tip = Jeter l'enregistrement (Échap)
bar-discard-confirm = Appuyer encore ou Échap pour le jeter
bar-record = Enregistrer
bar-record-tip = Démarrer l'enregistrement ({ $key })
bar-buffer = Tampon
bar-buffer-tip = Afficher ou masquer le tampon ({ $key })
bar-mic = Microphone
bar-mic-tip = Activer ou couper le microphone
bar-mic-level-tip = Niveau du microphone
bar-cam = Webcam
bar-cam-tip = Activer ou couper la webcam
bar-settings = Réglages
bar-collapse = Réduire
mode-region = Zone
mode-window = Fenêtre
mode-screen = Écran
mode-tip = { $mode } : { $key } capture tout de suite

## Notification (M2.6)

toast-annotate = Annoter
toast-annotate-tip = Dessiner sur cette capture
toast-undo = Annuler
toast-undo-tip = Retirer cette capture

## Tampon (M1)

tray-title = Tampon · { $count ->
    [one] { $count } capture
   *[other] { $count } captures
}
tray-title-selected = Tampon · { $count ->
    [one] { $count } capture
   *[other] { $count } captures
} · { $selected ->
    [one] { $selected } sélectionnée
   *[other] { $selected } sélectionnées
}
tray-empty = Vide — appuyez sur { $capture } ou { $record }. Les captures s'empilent ici aussitôt.
prefix-hint = sans préfixe
prefix-tip = Préfixe des nouveaux noms de fichier. Prochain : { $next }
folder-tip = Les nouvelles captures vont dans { $folder }. Cliquer pour choisir un autre dossier ({ $key })
rename-to-timestamp = l'horodatage seul
rename-question = Renommer { $count ->
    [one] la capture
   *[other] les { $count } captures
} du tampon en { $to } ?
rename-yes = Renommer
rename-yes-tip = Renommer les fichiers ; les captures en cours d'écriture gardent leur nom
rename-no = Garder les noms
rename-no-tip = Laisser leurs noms tels quels
card-saving = Enregistrement…
card-failed = Échec
card-pin = Épingler
card-pin-tip = Épingler : la garder au-delà de la durée de conservation
card-unpin-tip = Épinglée : gardée au-delà de la durée de conservation. Cliquer pour désépingler
card-drag-tip = Glisser dans une autre application pour partager le fichier
card-select-tip = Cliquer pour sélectionner, Maj ou { $key } pour ajouter
card-copy = Copier l'image
card-save = Enregistrer une copie sous…
card-annotate = Annoter
card-play = Lire
card-delete = Supprimer la capture et son fichier
# $count vaut 0 quand rien n'est sélectionné : l'action porte alors sur toutes les captures.
batch-copy = { $count ->
    [0] Tout copier
   *[other] Copier { $count }
}
batch-copy-tip = { $count ->
    [0] Copier toutes les captures
   *[other] Copier les { $count } sélectionnées
}
batch-save = { $count ->
    [0] Tout enregistrer…
   *[other] Enregistrer { $count }…
}
batch-save-tip = { $count ->
    [0] Enregistrer une copie de toutes les captures dans un dossier
   *[other] Enregistrer une copie des { $count } sélectionnées dans un dossier
}
batch-pdf-tip = { $count ->
    [0] Exporter toutes les captures en un seul PDF, une page par image
   *[other] Exporter les { $count } sélectionnées en un seul PDF, une page par image
}
batch-clear = { $count ->
    [0] Tout vider
   *[other] Vider { $count }
}
batch-clear-tip = { $count ->
    [0] Retirer toutes les captures du tampon ; les fichiers restent
   *[other] Retirer les { $count } sélectionnées du tampon ; les fichiers restent
}
batch-delete = Supprimer { $count }
batch-delete-tip = Supprimer les { $count } sélectionnées ; les fichiers vont à la Corbeille
batch-deselect = Désélectionner
batch-deselect-tip = Agir de nouveau sur toutes les captures
batch-ask = { $label } ?
batch-confirm = Cliquer encore pour confirmer
tray-menu-toggle = Afficher ou masquer la barre
tray-menu-quit = Quitter Capture Bar

## Sélection à l'écran

overlay-title = Sélection Capture Bar
overlay-record = Enregistrer  R
overlay-capture = Capturer  ⏎
hint-drag-region = Glisser pour une nouvelle zone
hint-click-window = Cliquer sur une fenêtre
hint-capture = Entrée ou double-clic pour capturer
hint-record = R enregistrer
hint-last-region = Espace dernière zone
hint-lock-ratio = Maj garder les proportions
hint-cancel = Échap annuler

## Éditeur d'annotations (US-7.1)

editor-title = Annoter
editor-title-file = Annoter · { $name }
tool-arrow = Flèche
tool-box = Cadre
tool-pen = Crayon
tool-text = Texte
tool-highlight = Surligner
tool-pixelate = Pixeliser
tool-crop = Recadrer
color-red = Rouge
color-yellow = Jaune
color-green = Vert
color-blue = Bleu
color-black = Noir
color-white = Blanc
editor-discard-question = Abandonner vos modifications ?
editor-discard = Abandonner
editor-discard-tip = Fermer sans enregistrer
editor-keep = Continuer
editor-keep-tip = Revenir aux annotations
editor-undo-tip = Annuler (Ctrl+Z)
editor-redo-tip = Rétablir (Ctrl+Y)
editor-original = Original
editor-original-tip = Revenir à la capture telle que prise ; Rétablir ramène les annotations
editor-save = Enregistrer
editor-save-tip = Enregistrer dans la capture (Ctrl+S)
editor-cancel = Annuler
editor-cancel-tip = Fermer sans enregistrer (Échap)

## Réglages (US-8.2)

settings-title = Réglages de Capture Bar
settings-output = Sortie
settings-folder = Dossier
settings-choose = Choisir…
settings-choose-tip = Aussi depuis le tampon de la barre ({ $key })
settings-prefix = Préfixe des noms de fichier
settings-recording = Enregistrement
settings-frame-rate = Images par seconde
settings-fps = { $rate } i/s
settings-audio = Audio
settings-system-audio = Son du système
settings-microphone = Microphone
settings-countdown = Compte à rebours
settings-countdown-tip = Secondes affichées à l’écran avant le début d’un enregistrement ; 0 démarre aussitôt
settings-general = Général
settings-autostart = Lancer Capture Bar à l'ouverture de session
settings-language = Langue
settings-language-auto = Celle du système
settings-buffer = Tampon
settings-keep = Garder les captures non épinglées
settings-days = { $count ->
    [one] jour
   *[other] jours
}
settings-keep-tip = Leurs fichiers restent dans le dossier
settings-file = Enregistrés dans { $file }

## Messages

toast-no-shortcuts = Raccourcis globaux indisponibles : utilisez la barre ou l'icône de notification
toast-settings-damaged = Le fichier de réglages était endommagé : valeurs par défaut
toast-index-damaged = L'index du tampon était endommagé : tampon vide
toast-recorded = Enregistrement sauvegardé ({ $seconds } s)
toast-record-failed = Échec de l’enregistrement : { $error }
toast-play-failed = Impossible de lire l’enregistrement : { $error }
toast-record-discarded = Enregistrement jeté
toast-region-off-screen = Échec de la capture : la zone est hors de l'écran
toast-capture-failed = Échec de la capture : { $error }
toast-added = Ajoutée au tampon · { $count }
toast-copied = Copié
toast-copied-files = { $count ->
    [one] { $count } fichier copié
   *[other] { $count } fichiers copiés
}
toast-copy-failed = Copie impossible : { $error }
toast-nothing-to-copy = Rien à copier pour l'instant
toast-nothing-saved = Aucune capture enregistrée à copier
toast-nothing-to-export = Aucune image enregistrée à exporter
toast-deleted = Supprimée
toast-deleted-count = { $count ->
    [one] { $count } supprimée
   *[other] { $count } supprimées
}
toast-trashed-one = Supprimée · { $file } est dans la Corbeille
toast-trashed = { $count } supprimées · les fichiers sont dans la Corbeille
toast-trash-failed = Retirée du tampon · des fichiers restent : { $error }
toast-cleared = { $count ->
    [one] { $count } retirée du tampon · le fichier reste
   *[other] { $count } retirées du tampon · les fichiers restent
}
toast-drag-failed = Glisser impossible : { $error }
toast-saved-copy = Copie enregistrée sous { $file }
toast-save-copy-failed = Impossible d'enregistrer une copie : { $error }
toast-saved-all = { $count ->
    [one] { $count } copie enregistrée dans { $folder }
   *[other] { $count } copies enregistrées dans { $folder }
}
toast-saved-some = { $count } copies enregistrées dans { $folder } ; { $failed } en échec
toast-exported = { $count ->
    [one] { $count } page exportée dans { $file }
   *[other] { $count } pages exportées dans { $file }
}
toast-export-failed = Export du PDF impossible : { $error }
toast-renamed = Renommées
toast-renamed-some = Renommées, sauf { $count } fichiers
toast-editor-busy = Enregistrez ou fermez d'abord l'annotation ouverte
toast-annotate-images-only = Seules les images peuvent être annotées
toast-not-ready = Cette capture n'est pas encore prête
toast-open-failed = Impossible d'ouvrir la capture : { $error }
toast-annotate-removed = La capture a été retirée : les annotations n'ont pas été enregistrées
toast-annotate-unsaved = La capture n'a pas été enregistrée, ses annotations non plus
toast-reverted = Retour à l'original · copié
toast-annotations-saved = Annotations enregistrées · copié
toast-annotate-failed = Impossible d'enregistrer les annotations : { $error }
toast-saved-not-copied = Enregistrée, mais pas copiée dans le presse-papiers
toast-save-failed = Impossible d'enregistrer la capture : { $error }
toast-autostart-failed = Lancement à l'ouverture de session inchangé : { $error }
dialog-save-all = Enregistrer { $count } captures dans…
dialog-export-pdf = Exporter { $count } images en un seul PDF
dialog-pdf-filter = Document PDF
dialog-save-copy = Enregistrer une copie de la capture
dialog-png-filter = Image PNG
dialog-folder = Dossier des nouvelles captures

countdown-title = Début de l’enregistrement
countdown-cancel = { $key } pour annuler
