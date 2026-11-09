# Capture Bar — English. Each message here must also be in fr.ftl (a test checks it).
# Variables: $key is a shortcut such as Ctrl+Shift+S; $count a number.

## Bar (M2.1, M2.2, M2.3)

grip-label = Move the bar
grip-tip = Drag to move. Double-click to dock
pill-expand = Expand the bar
bar-capture = Capture
bar-capture-tip = Capture with the current mode ({ $key })
bar-repeat-tip = Capture the same region again ({ $key })
bar-stop = Stop
bar-stop-tip = Stop recording ({ $key })
bar-pause = Pause
bar-pause-tip = Pause recording ({ $key })
bar-resume = Resume
bar-resume-tip = Resume recording ({ $key })
bar-discard = Discard
bar-discard-tip = Throw the recording away (Esc)
bar-discard-confirm = Press again or Esc to throw it away
bar-record = Record
bar-record-tip = Start recording ({ $key })
bar-buffer = Buffer
bar-buffer-tip = Show or hide the buffer ({ $key })
bar-mic = Microphone
bar-mic-tip = Microphone on or off
bar-mic-level-tip = Microphone level
bar-cam = Webcam
bar-cam-tip = Webcam on or off
bar-settings = Settings
bar-collapse = Collapse
mode-region = Region
mode-window = Window
mode-screen = Screen
mode-tip = { $mode }: { $key } captures now

## Toast (M2.6)

toast-annotate = Annotate
toast-annotate-tip = Draw on this capture
toast-undo = Undo
toast-undo-tip = Remove this capture

## Tray (M1)

tray-title = Buffer · { $count ->
    [one] 1 capture
   *[other] { $count } captures
}
tray-title-selected = Buffer · { $count ->
    [one] 1 capture
   *[other] { $count } captures
} · { $selected } selected
tray-empty = Empty — press { $capture } or { $record }. Captures stack up here instantly.
prefix-hint = no prefix
prefix-tip = Prefix of new file names. Next: { $next }
folder-tip = New captures go to { $folder }. Click to choose another folder ({ $key })
rename-to-timestamp = the timestamp alone
rename-question = Rename the { $count ->
    [one] 1 capture
   *[other] { $count } captures
} in the buffer to { $to }?
rename-yes = Rename
rename-yes-tip = Rename the files; captures still being written keep their name
rename-no = Keep names
rename-no-tip = Leave their names as they are
card-saving = Saving…
card-failed = Failed
card-pin = Pin
card-pin-tip = Pin: keep it past the retention period
card-unpin-tip = Pinned: kept past the retention period. Click to unpin
card-drag-tip = Drag into another app to share the file
card-select-tip = Click to select, Shift or { $key } to add
card-copy = Copy the image
card-save = Save a copy as…
card-annotate = Annotate
card-play = Play
card-delete = Delete the capture and its file
# $count is 0 when nothing is selected: the action is then on every capture.
batch-copy = { $count ->
    [0] Copy all
   *[other] Copy { $count }
}
batch-copy-tip = { $count ->
    [0] Copy every capture
   *[other] Copy the { $count } selected
}
batch-save = { $count ->
    [0] Save all…
   *[other] Save { $count }…
}
batch-save-tip = { $count ->
    [0] Save copies of every capture into a folder
   *[other] Save copies of the { $count } selected into a folder
}
batch-pdf-tip = { $count ->
    [0] Export every capture as one PDF, a page per image
   *[other] Export the { $count } selected as one PDF, a page per image
}
batch-clear = { $count ->
    [0] Clear all
   *[other] Clear { $count }
}
batch-clear-tip = { $count ->
    [0] Take every capture out of the buffer; the files stay
   *[other] Take the { $count } selected out of the buffer; the files stay
}
batch-delete = Delete { $count }
batch-delete-tip = Delete the { $count } selected; the files go to the Recycle Bin
batch-deselect = Deselect
batch-deselect-tip = Act on every capture again
batch-ask = { $label }?
batch-confirm = Click again to confirm
tray-menu-toggle = Show or hide the bar
tray-menu-quit = Quit Capture Bar

## Selection overlay

overlay-title = Capture Bar selection
overlay-record = Record  R
overlay-capture = Capture  ⏎
hint-drag-region = Drag for a new region
hint-click-window = Click a window
hint-capture = Enter or double-click capture
hint-record = R record
hint-last-region = Space last region
hint-lock-ratio = Shift lock ratio
hint-cancel = Esc cancel

## Annotation editor (US-7.1)

editor-title = Annotate
editor-title-file = Annotate · { $name }
tool-arrow = Arrow
tool-box = Box
tool-pen = Pen
tool-text = Text
tool-highlight = Highlight
tool-pixelate = Pixelate
tool-crop = Crop
color-red = Red
color-yellow = Yellow
color-green = Green
color-blue = Blue
color-black = Black
color-white = White
editor-discard-question = Discard your changes?
editor-discard = Discard
editor-discard-tip = Close without saving
editor-keep = Keep editing
editor-keep-tip = Back to the annotations
editor-undo-tip = Undo (Ctrl+Z)
editor-redo-tip = Redo (Ctrl+Y)
editor-original = Original
editor-original-tip = Back to the capture as taken; Redo brings the annotations back
editor-save = Save
editor-save-tip = Save into the capture (Ctrl+S)
editor-cancel = Cancel
editor-cancel-tip = Close without saving (Esc)

## Settings (US-8.2)

settings-title = Capture Bar settings
settings-output = Output
settings-folder = Folder
settings-choose = Choose…
settings-choose-tip = Also from the bar's tray ({ $key })
settings-prefix = File name prefix
settings-recording = Recording
settings-frame-rate = Frame rate
settings-fps = { $rate } fps
settings-audio = Audio
settings-system-audio = System sound
settings-microphone = Microphone
settings-countdown = Countdown
settings-countdown-tip = Seconds shown on screen before a recording starts; 0 starts at once
settings-general = General
settings-autostart = Start Capture Bar when I log in
settings-language = Language
settings-language-auto = Same as the system
settings-buffer = Buffer
settings-keep = Keep unpinned captures for
settings-days = { $count ->
    [one] day
   *[other] days
}
settings-keep-tip = Their files stay in the folder
settings-file = Saved in { $file }

## Messages

toast-no-shortcuts = Global shortcuts unavailable: use the bar or the tray icon
toast-settings-damaged = The settings file was damaged: defaults used
toast-index-damaged = The buffer index was damaged: started empty
toast-recorded = Recording saved ({ $seconds } s)
toast-record-failed = Recording failed: { $error }
toast-play-failed = Could not play the recording: { $error }
toast-record-discarded = Recording discarded
toast-region-off-screen = Capture failed: the region is off screen
toast-capture-failed = Capture failed: { $error }
toast-added = Added to buffer · { $count }
toast-copied = Copied
toast-copied-files = Copied { $count } files
toast-copy-failed = Could not copy: { $error }
toast-nothing-to-copy = Nothing to copy yet
toast-nothing-saved = Nothing saved to copy yet
toast-nothing-to-export = No saved image to export yet
toast-deleted = Deleted
toast-deleted-count = Deleted { $count }
toast-trashed-one = Deleted · { $file } is in the Recycle Bin
toast-trashed = Deleted { $count } · the files are in the Recycle Bin
toast-trash-failed = Removed from the buffer · some files stay: { $error }
toast-cleared = Cleared { $count } from the buffer · the files stay
toast-drag-failed = Drag out: { $error }
toast-saved-copy = Saved a copy as { $file }
toast-save-copy-failed = Could not save a copy: { $error }
toast-saved-all = Saved { $count } copies in { $folder }
toast-saved-some = Saved { $count } copies in { $folder }; { $failed } failed
toast-exported = Exported { $count } pages to { $file }
toast-export-failed = Could not export the PDF: { $error }
toast-renamed = Renamed
toast-renamed-some = Renamed, but { $count } files could not be
toast-editor-busy = Save or close the open annotation first
toast-annotate-images-only = Only images can be annotated
toast-not-ready = This capture is not ready yet
toast-open-failed = Could not open the capture: { $error }
toast-annotate-removed = The capture was removed: the annotations were not saved
toast-annotate-unsaved = The capture was not saved, so its annotations cannot be
toast-reverted = Back to the original · copied
toast-annotations-saved = Annotations saved · copied
toast-annotate-failed = Could not save the annotations: { $error }
toast-saved-not-copied = Saved, but not copied to the clipboard
toast-save-failed = Could not save the capture: { $error }
toast-autostart-failed = Start at login not changed: { $error }
dialog-save-all = Save { $count } captures into…
dialog-export-pdf = Export { $count } images as one PDF
dialog-pdf-filter = PDF document
dialog-save-copy = Save a copy of the capture
dialog-png-filter = PNG image
dialog-folder = Folder for new captures

countdown-title = Recording starts
countdown-cancel = Press { $key } to cancel

bubble-title = Camera
bubble-starting = Starting camera…
toast-camera-failed = Camera unavailable: { $error }
