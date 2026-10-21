# FL Studio parity checklist

What FL Studio (21 to 24) has, and where Lumen Audio Studio stands. The goal is FL's
workflow and layout, in our own design and with our own names. Nothing here copies FL's
artwork, names of its plugins, or its file formats.

Status: **done**, **partial** (works but is missing parts FL has), **missing**.
Priority: **P1** (core workflow, people notice at once), **P2** (expected by regular
users), **P3** (nice to have). The batches at the end are the order of the work.

Last checked: 0.2.0-alpha.49.

## 1. Top bar and transport

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Pattern / Song mode switch | done | | |
| Play, stop, record buttons, Space to play | done | | |
| Tempo display with drag and tap | done | | |
| Song position display (bar:beat:step) | done | | Also time (min:sec) on click: missing, P3 |
| Metronome | done | | Count-in before recording: missing, P2 |
| Pattern selector with new pattern button | done | | |
| CPU and memory meter | done | | |
| Master volume and master pitch knobs | partial | P3 | Volume only |
| Snap selector in the top bar (global snap) | missing | P2 | Each editor has its own snap |
| Recording options (audio, notes, automation, loop record) | partial | P2 | Audio only |
| Typing keyboard to piano | done | | |
| Hint bar (what is under the mouse) | missing | P2 | |
| Online/news panel | n/a | | Not wanted |

## 2. Browser

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Samples tree with search and preview on click | done | | |
| Plugins tab (instruments, effects, hosted plugins) | done | | |
| Presets tab | done | | |
| Current project tab (patterns, channels, samples, automation) | partial | P2 | Lists some, cannot drag from it |
| Drag a sample onto the rack, playlist or a sampler | partial | P1 | Only file drops from the system |
| Drag a preset onto a channel | missing | P2 | |
| Favourites and user folders | partial | P2 | Import only, no favourites |
| Tags and smart search | missing | P3 | |

## 3. Channel rack

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Step sequencer with mute, level, pan, name, steps | done | | |
| Per channel swing | done | | |
| Fill each N steps, shift, randomise, clear | done | | |
| Channel colours, rename, clone, delete, route to insert | done | | |
| Channel groups (filter the rack by group) | missing | P2 | |
| Graph editor under the steps (velocity, pan, pitch per step) | missing | P1 | |
| Pattern length per pattern (beyond 16 steps), step count per channel | partial | P1 | Check what the rack allows |
| Mini piano roll preview on channels with notes | missing | P2 | |
| Channel selection and solo (alt click) | partial | P2 | |
| Target mixer track number on each channel | partial | P2 | Through the menu only |
| Layer channel (play several channels from one) | missing | P3 | |
| Zip/unzip channels | missing | P3 | |

## 4. Piano roll

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Draw, select, delete, slice, mute tools | partial | P1 | Slice and mute in the roll are missing |
| Paint tool (drag to draw many notes) | missing | P1 | |
| Velocity lane | done | | |
| Other lanes: pan, release, fine pitch, mod X/Y, any automation | missing | P1 | |
| Note slides and porta | missing | P3 | |
| Ghost notes | done | | |
| Scale highlight and snap to scale | done | | |
| Chord stamps | done | | |
| Tools: legato, strum, flip, quantize, humanize, chop, arpeggiate, randomise, LFO, riff machine | partial | P2 | Arpeggiate, LFO tool, riff generator missing |
| Note colours / MIDI channels | missing | P3 | |
| Zoom with ctrl+wheel, scroll, keyboard on the left that plays | partial | P1 | Check zoom and clickable keys |
| Time markers and loop in the roll | missing | P2 | |
| Detach / multiple roll windows | missing | P3 | |
| Note properties dialog (double click a note) | missing | P2 | |
| Copy, paste, duplicate (ctrl+B) | partial | P1 | Duplicate missing |

## 5. Playlist

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Pattern, audio and automation clips | done | | |
| Draw, select, slice, mute, delete tools | done | | |
| Paint tool | missing | P1 | |
| Clip colours, make unique | done | | |
| Markers, song loop, export the loop | done | | |
| Track height, fit, zoom | done | | |
| Track names, colours, mute | partial | P2 | Solo and lock missing |
| Clip resize from the start (trim left), clip stretch | partial | P1 | |
| Picker panel (patterns and clips to drag in) | missing | P1 | |
| Time signature changes, tempo automation | partial | P2 | Tempo changes exist; signature missing |
| Performance mode | missing | P3 | |
| Audio clip fades, gain, reverse, time stretch, pitch | partial | P1 | See section 8 |
| Crossfades on overlapping audio | missing | P3 | |
| Grouping clips | missing | P3 | |

## 6. Mixer

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Inserts with name, colour, pan, mute, solo, fader, meter | done | | |
| Ten effect slots per insert, on/off, mix per slot | done | | |
| Routing and sends, sidechain-only sends | done | | |
| Stereo width per insert | done | | |
| Stereo separation, swap, polarity, delay compensation per insert | partial | P2 | Width only |
| Insert EQ (3 band) built into each strip | missing | P2 | |
| Peak meters with hold, clip indicator | partial | P2 | |
| Disk recording per insert (audio input select) | missing | P2 | |
| Docking channels to left / right sides, wide/compact views | missing | P3 | |
| Effect slot drag to reorder, save effect chains | missing | P2 | |
| Plugin delay compensation | done | | |
| Link selected channels to an insert from the mixer | missing | P2 | |

## 7. Plugins and plugin hosting

| Feature | Status | Priority | Notes |
|---|---|---|---|
| CLAP and VST3 instruments and effects | done | | |
| VST3 plugin windows | done | | |
| CLAP plugin windows | missing | P1 | |
| Run plugins in a separate process (crash safety) | missing | P1 | |
| Plugin picker with categories and a database | partial | P2 | |
| Plugin presets (save and load .fst-like, our own format) | partial | P2 | Built-ins only |
| Wrapper menu: MIDI settings, latency, processing options | missing | P3 | |
| Plugin delay compensation for hosted plugins | partial | P2 | |
| Multi-output plugins to several inserts | missing | P3 | |
| Sidechain into hosted plugins | missing | P3 | |

## 8. Audio editing

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Recording audio | done | | |
| Audio clip editor: trim, fade in/out, gain, normalise, reverse | partial | P1 | |
| Time stretch and pitch shift of clips (keep tempo) | partial | P2 | Stretch code exists in las-dsp |
| Sample editor (cut, copy, paste inside a sample, regions) | missing | P2 | |
| Slicer instrument (chop a loop to pads/notes) | missing | P2 | |
| Pitch correction | missing | P3 | |

## 9. Automation

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Automation clips from any knob | done | | |
| Curve editing with points | done | | |
| Curve shapes per segment (hold, smooth, pulse, wave) | missing | P2 | |
| LFO tool on a clip | missing | P3 | |
| Record automation from knob moves | missing | P2 | |
| Event editor (per pattern) | missing | P3 | |
| MIDI learn and controller linking | missing | P1 | |

## 10. Instruments and effects

| Feature | Status | Priority | Notes |
|---|---|---|---|
| Sampler with ADSR, filter | done | | |
| Drum pads | done | | |
| Synths (wavetable, FM, granular, modular, layered ROM player) | done | | Being redesigned now |
| 3 oscillator synth in the FL style | partial | | TriOsc, being redesigned now |
| Slicer, beat slicer | missing | P2 | |
| Effects: EQ, compressor, limiter, reverb, delay, chorus, flanger, phaser, distortion, gate | done | | |
| Multiband compressor, maximiser | partial | P2 | |
| Vocoder | missing | P3 | |
| Spectrum analyser, oscilloscope | partial | P2 | |
| Tuner | missing | P3 | |

## 11. MIDI and hardware

| Feature | Status | Priority | Notes |
|---|---|---|---|
| MIDI keyboard input | done | | |
| Record notes from a MIDI keyboard into the piano roll | missing | P1 | |
| MIDI learn (link a knob to a controller) | missing | P1 | |
| MIDI file import into the piano roll | missing | P2 | Export exists |
| MIDI out to hardware | missing | P3 | |
| Audio device settings (device, buffer size, sample rate) | missing | P1 | Uses the default device |

## 12. Project and app

| Feature | Status | Priority | Notes |
|---|---|---|---|
| New, open, save, save as, demo song, autosave | partial | P2 | Autosave and backups missing |
| Undo history | done | | |
| Export WAV, FLAC, MP3, MIDI, the loop | done | | |
| Export stems (each insert to its own file) | missing | P1 | |
| Project info (title, author, notes) | missing | P3 | |
| Templates | missing | P2 | |
| Settings window (audio, MIDI, general, file paths) | missing | P1 | |
| Themes | done | | |
| Keyboard shortcuts list | done | | |
| Window layouts that are remembered | missing | P2 | |
| Touch-friendly and detached windows | missing | P3 | |

## Order of the work

1. **Batch A, settings and hardware:** settings window with audio device, buffer size and
   sample rate, MIDI input choice; record notes from a MIDI keyboard; MIDI learn for
   knobs; export stems.
2. **Batch B, editors:** paint tool in the roll and playlist; duplicate (ctrl+B); note
   properties; piano roll lanes for pan and fine pitch; clip trim from the start; picker
   panel in the playlist.
3. **Batch C, rack:** graph editor under the steps; pattern length; channel groups; drag
   samples and presets from the browser.
4. **Batch D, audio clips:** fades, gain, reverse, normalise, time stretch and pitch.
5. **Batch E, mixer:** built-in EQ per insert, polarity, swap and separation, effect
   slot reordering and chains, peak hold.
6. **Batch F, automation:** segment curve shapes, recording knob moves, MIDI import.
7. The P3 items, after the above.

Plugin hosting (CLAP windows, separate process) runs in parallel with batch A.
