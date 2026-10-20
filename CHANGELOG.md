# Changelog

Each version that lands on `main` is published as a GitHub Release. Versions with a `-` (for example `-alpha.1`) are pre-releases.

## 0.2.0-alpha.25

More of an FL Studio workflow.

- Import your own samples: File > Import samples..., the Import button in the browser, or drop WAV files on the window. Each becomes a sampler channel with its own insert, and the file is stored in the project. Double click a factory sample in the browser to add it as a channel (a single click still plays it).
- Metronome: a button next to Stop; a click on every beat (higher on the first of the bar) while the project plays.
- Piano roll: a velocity lane under the grid; drag in it to set the velocity of the notes above.
- Mixer routing: choose what an insert plays to, and add sends with their own level and a sidechain-only switch. Loops are refused.

## 0.2.0-alpha.24

Menus, new and demo projects, and FL Studio style tools.

- The app opens on the "new project" of the web version: Kick, Clap, Hat and Snare samplers and a Nebula, a four on the floor pattern in the playlist. `lumen song.las` opens that file instead.
- Main menu (the button at the left of the top bar): File (New, Open, Open demo song, Save, Save as, Export, Quit), Edit (undo, redo), Add (any instrument as a new channel on a free insert), Patterns (new, clone, delete), View, Options (themes) and Help (About). Native file dialogs for Open and Save as; a question about unsaved changes before anything that would lose them. Shortcuts: Ctrl+N, Ctrl+O, Ctrl+S, Ctrl+Shift+S, Ctrl+R, F4 (new pattern), F5 to F9 (windows), Space (play and stop).
- The rack has an "Add channel" menu.
- Piano roll: Draw, Select and Delete tools, Quantize, and ghost notes (the other channels' notes behind the one being edited).
- Playlist: Draw, Select, Slice, Mute and Delete tools; slicing cuts a clip at the grid line, mute switches a clip off.
- Mixer: every insert is shown with its coloured name, pan, mute, solo and fader; the selected insert has a panel with its name, its ten effect slots (add an effect to any slot) and its stereo width.
- Builds on pull requests no longer upload packages (only a release does, kept for a day), which stops the runs from filling the repository's Actions storage.

## 0.2.0-alpha.23

Effect windows for the mixer.

- Click an effect in a mixer strip to open its window: the switch, presets, mix, and every parameter, with the same curves and knobs as the instrument racks (the equaliser's curve, the gate's steps, the shaper and step effect editors). Edits go to the project through undo and are saved with it.
- A "+" on each strip adds any effect to the first free slot; right click an effect to remove it; the dot beside it switches it on or off.
- `lumen song.las --effect 1,0` opens the effect in slot 0 of insert 1 at the start.

## 0.2.0-alpha.22

Instrument windows for the project's channels.

- Double click a channel's name in the rack to open its instrument. Facet, Mist and the wavetable family (Polaris, Vortex, Monolith, Zenith, TriOsc) open in the synth panels with the channel's own patch and effects loaded, and every edit, preset change and effect change is written back to the channel through undo and saved with the project; the next play uses it. Right click a name for its piano roll.
- Instruments without a panel yet (Nebula, Sampler, LumaPad) open a basic editor that lists the channel's settings (numbers, switches and text) and writes them back the same way.
- A knob drag is one undo step, not one per movement.

## 0.2.0-alpha.21

The window now looks and works like the web version.

- **Themes:** all eight of the web version's themes (Lumen Dark, Orange Steel, Daylight, Synthwave, Forest, Ocean, Graphite, High Contrast), derived from the same seed colours with the same formulas, plus "Match the system". Choose one in the menu; it is remembered. Every widget follows it.
- **Top bar:** logo and menu, PAT and SONG, play and stop (Space also plays and stops), the tempo display (drag it, or tap a tempo), the BAR:BEAT:STEP display, the pattern chooser with a button for a new pattern, toggles for the windows, Save, Export, and the master knob with its meters.
- **Browser** on the left with the web version's four tabs: Samples (the factory kit in folders, with search, and click to audition a sample through the output; the project's own samples), Plugins (instruments and effects), Presets, and Project (open a file, its details).
- **Windows** that float over the workspace like the web version's: Playlist, Channel Rack, Piano Roll, Mixer and Synth, each draggable, resizable and closable. `--open roll,mixer,synth,export` and `--close playlist,rack,browser` choose which are open at the start.
- **Channel rack as a step sequencer:** a mute light, level and pan knobs, the channel name, and a cell for every step; click a cell to place or remove a step and drag to paint. A double click on a name opens its piano roll. Swing sits at the bottom. All of it joins undo, redo and save.
- **Export window:** WAV at 16, 24 or 32 bit or FLAC at 16 or 24 bit, normalised if wanted, the song or the chosen pattern, rendered in the background.
- **Status bar** with the project name, the keyboard octave, the audio device and DSP load, voices and dropouts.

## 0.2.0-alpha.20 - 2026-10-20

Editing while the song plays.

- An edit made while a song or pattern plays now takes effect: a player for the edited project is built in the background (once the pointer is up) and takes over from the position the old one had reached. The sounds still ringing from the old player stop at the swap.

## 0.2.0-alpha.19 - 2026-10-20

The mixer, and a tighter project page.

- A mixer page with a strip for the master and every insert that is in use: fader (with dB), pan, mute and solo, and the effects in its slots, each of which can be switched on or off with a click. Edits join the same undo, redo and save as the other editors.
- The project page header is two rows instead of five, which leaves the room to the editors. `lumen song.las --mixer` starts on the mixer.

## 0.2.0-alpha.18 - 2026-10-20

The playlist.

- A playlist page lays the song out in tracks of clips (patterns, audio and automation, each with its name and colour). Double click places the chosen pattern on the bar, drag moves a clip in time and between tracks, drag an edge resizes, right click deletes, and a click on the ruler sets where "Play song" starts. Edits join the same undo, redo and save as the piano roll.
- `lumen song.las --roll` and `--playlist` start on those pages; a project opened from the command line fills in the path box.

## 0.2.0-alpha.17 - 2026-10-20

The piano roll, and saving.

- A piano roll for the chosen channel and pattern, next to the channel rack. Click an empty cell to place a note (it sounds as you place it), drag on empty space to place one and stretch it, drag a note to move it (in time and pitch), drag its right edge to resize it, right click to delete. Snap to a beat, a half, a quarter or an eighth of a beat, or off; zoom; the keys stay in view while the roll scrolls.
- Undo and redo (Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y) for every edit, and Save (Ctrl+S) writes the project back to its `.las` file, which the web version reads.
- After an edit, a fresh player is built in the background when the pointer is up and nothing is playing, so the keys and the next play use the edited notes. Editing while playing is not supported yet.

## 0.2.0-alpha.16 - 2026-10-20

The channel rack.

- The Project page draws every channel with its notes in the chosen pattern: step notes as cells, other notes as bars placed by key and sized by length and velocity, with beat and bar lines.
- While a pattern plays, a playhead moves across the strips. Click a channel to choose the one the keys play.

## 0.2.0-alpha.15 - 2026-10-20

The app opens and plays a project.

- `lumen song.las` (or a `.las` file dropped on the window, or a path typed on the new Project page) opens a project. The page shows its tempo, channels and patterns.
- Play song, Play pattern (loops the chosen pattern) and Stop. The position shows as bar:beat:tick.
- The sounds of a project are loaded on a worker thread, and the audio thread then takes the player over through the lock-free queue; the old player is handed back to be freed off the audio thread.
- Pick a channel and play it with the computer keyboard or the piano, through the project's own mixer and effects.

## 0.2.0-alpha.14 - 2026-10-20

A live player.

- `Player` in `las-engine` plays a project in real time: it schedules a short way ahead, renders 128 frame blocks, and takes live notes on any channel. It shares its scheduler and block renderer with the offline render (`Engine`), so a project sounds the same played or rendered.
- Time is reckoned from the start of the song rather than from each chunk, so the tempo cells, ramps and envelopes land on the same frames however the song is cut up.
- Checked in `tests/player.rs`: every parity fixture, played in blocks, matches its offline render (to rounding for most; the LumaPad choke case is left out, because the browser's offline render also chokes later pads that are already scheduled, which a lookahead player cannot do).
- Not yet: the player allocates while rendering, and a project edited while playing needs a new player.

## 0.2.0-alpha.13 - 2026-10-20

Export, and wavetables made from samples.

- `las midi song.las` writes the notes as a type 1 standard MIDI file, one track per channel with notes, byte for byte the file the web version writes (checked on the demo song, whole and one pattern).
- `las render-project` writes WAV at 16, 24 or 32 bit (`--bits`, with the web version's dither at 16 bit) or FLAC at 16 or 24 bit (`--format flac`), can scale the peak to -0.3 dB (`--normalize`), and with `--stems <dir>` writes the master and each used insert after its fader, each trimmed by its own plugin delay (checked against the web stems).
- A wavetable made from a stored sample (`user:<sample>` in a patch, as in Vortex) plays: the sample is cut into 2048 sample frames and turned into a table. The synths keep the project's tables when automation changes their patch.
- Not yet: MP3 export, and automation of the maximizer.

## 0.2.0-alpha.12 - 2026-10-20

Automation of effect and instrument parameters.

- Automation clips now drive effect parameters and a slot's mix, as the web version does: ramps read once per render block for the dynamics, limiter, crusher, gate, shaper, step and DSP-module effects; the slot's mix as wet and dry gain ramps; the equaliser's frequency, gain, Q and output as parameter ramps; and everything else as state updates on a 30 ms grid.
- The effects that the browser builds from Web Audio nodes (equaliser, overdrive, chorus, phaser, auto filter, stereo tool, reverb) now move to a changed setting the way the nodes do: with a 15 ms time constant, so an edit while playing no longer clicks. A filter forgets what it heard when its type changes.
- The parameter timeline learned two more things about Chromium, found by measuring it: a node that processes two channels asks for each parameter once per channel, and a target curve carries on from one request to the next (so the right channel of a filter runs a render quantum ahead of the left); and a target counts as reached within 1.5 sixteen-bit steps of its value, so large values such as a cutoff in Hz snap later than gains.
- A reverb whose response does not change keeps its tail when a setting moves.
- Checked against the web render: ramps and the slot mix exact to rounding; the other automated effects 98 to 125 dB below the signal (`web_parity.rs`).
- Instrument parameters can be automated too: the synths' patch values (filter, envelopes, levels, LFOs and the rest, read every 32 frames as in the browser), Facet and Mist, Nebula's macros, master, layer level, pan and layer patches, the effects in an instrument's own rack (including their mix), the sampler (cutoff, Q and boost as parameter ramps, tuning and envelope read when a note starts) and an audio clip channel's gain. Checked against the web render: Polaris, Facet and Mist exact, the others 111 to 130 dB below the signal.
- LumaPad plays: 16 pads with their own samples, level, pan and tune, choke groups, and their automation. The sampler's voices are shared with it.
- The whole demo song renders (`las render-project demo.las`). Against the browser, the drums, Facet lead, riser, crash and impact agree to 100 dB; the Polaris and Zenith parts agree in level to 1% (the browser's unison and random phase are not reproducible, so they cannot be compared sample by sample).
- Not yet: automation of the maximizer.

## 0.2.0-alpha.11 - 2026-10-16

Audio clips, automation of the mixer and channels, and tempo changes.

- Audio clips in the playlist play: clip gain, fades of any shape, automatic crossfades where clips overlap, reverse, pitch shift, and tempo follow by time stretch or by resampling. The time stretch (WSOLA with transient locking) is a port of the web version's and matches it to rounding; a mono clip pans with the mono law, as in the browser. Checked against the web render of a project with seven clips.

- Automation clips in the playlist drive insert volume and pan, master volume, channel volume, pan and pitch, with the web version's timing: segments are cut at the render's tempo cells, a clip that starts mid-way jumps to its value, points move later by the plugin delay of the insert they control, and clip offsets and partial overlaps work.
- Tempo automation (the `master tempo` clips) changes the speed of the whole render, including the arpeggiator and the length of the file.
- Checked against the web version: the automation and tempo projects match with no sample differing by more than 2e-6.
- Not yet: automation of effect and instrument parameters.

## 0.2.0-alpha.10 - 2026-10-16

The sampler and the factory sample kit.

- The 29 factory samples (kicks, snares, claps, hats, cymbals, percussion, 808s, risers, impact, stabs) are synthesised in Rust and match the web version's to better than 2e-6.
- The sampler instrument plays them in a project render: pitch from the key, tune and fine tune, start and end, loop, reverse, choke, velocity to volume, attack, decay, sustain and release, filter and boost, pan. Samples stored in a project file (WAV) play too; Mist uses a channel's own sample when it has one.
- The Web Audio parts behave as Chromium runs them, found by measuring it: a buffer source starts between frames and reads with linear interpolation (carrying the last slope past the end of the buffer), a stop time falls on the frame the browser computes in `f32`, and an `AudioParam` timeline is evaluated per frame (ramps, `setTargetAtTime` that starts from the last frame before it, and the quantum at which a finished target snaps to its value).
- Checked against the web render: the sampler project agrees to 113 dB below the signal; kick, reverse, loop and choke cases to rounding.

## 0.2.0-alpha.9 - 2026-10-15

Play a project: the scheduler, the mixer and an offline render.

- New crate `las-engine`. It schedules a project's notes (patterns, the playlist, swing, tempo automation, the arpeggiator), runs each channel's instrument (Polaris, Vortex, Monolith, Zenith, TriOsc, Nebula with its layers and macros, Facet, Mist) through its effects rack, and mixes the inserts: faders, pan, width, mute and solo, effect slots, sends, routing, sidechain (chosen source or an insert routed "sidechain only"), and plugin delay compensation.
- `las render-project song.las -o song.wav` renders the whole song, or one pattern with `--pattern`, to a 32 bit float WAV.
- Checked against the web version by rendering the same projects in headless Chromium and comparing: the instruments, the mixer, routing, sidechain and the arpeggiator match to rounding (more than 120 dB below the signal), and the effects Chromium builds from Web Audio nodes match to better than 85 dB.
- To get there the Web Audio based effects were corrected against the browser: the overdrive uses Chromium's 4x oversampling filters and their 192 sample delay, the maximizer uses a port of Chromium's dynamics compressor (look-ahead, adaptive release), biquads run in direct form I with `f32` between nodes, the chorus reads its delay line with linear interpolation and feeds back one render quantum late, and the phaser's feedback delay is 128 samples (it was 129). Cutoffs at 0 or at the Nyquist frequency behave as in the browser.
- Not yet in the render: the sampler and its factory samples, audio clips, LumaPad, and automation clips.

## 0.2.0-alpha.8 - 2026-10-15

Projects: the model and the `.las` file format.

- New crate `las-project`: the project model (channels, patterns, notes, playlist, automation, mixer with effect slots and sends, samples, markers, MIDI map) as the web version saves it. Fields the model does not know are kept, so a save and reopen loses nothing.
- Open `.las` files from the web version, encrypted or plain, with their embedded samples. Files written here open in the web version. The file is checked three ways: the authentication tag, the CRC and the length, and each failure says what is wrong.
- Migration and repair are ported: version 1 projects are updated, and bad tempo, time signature, routing loops, dangling sends, clips, automation and MIDI assignments, duplicate ids and out of range values are repaired, each reported as a note. Checked against the web version on the demo song and on a deliberately damaged project (same repairs, same notes, same order).
- `las info project.las` lists what a project holds. `las convert in.las out.las [--plain]` reads and writes it again.

## 0.2.0-alpha.7 - 2026-10-15

The effects rack.

- Eight more effects, completing the catalogue: parametric EQ (seven bands), overdrive, chorus, phaser, auto filter, stereo tool, maximizer (three-band) and convolution reverb. The web version builds these from Web Audio's own filter, delay, wave shaper and convolver nodes, which are not specified to the sample, so they are written to behave like those nodes (biquad formulas, wave shaper curve, convolver normalisation) and read the same settings.
- Every instrument has an effects rack in the app (Effects tab): add, remove and reorder effects, enable and mix each one, pick effect presets (80 from the web version), and tempo for the synced ones. Controls are drawn from a catalogue of the effects (ranges, curves, units, options), with an EQ response curve, trance gate steps, volume shaper curves and step FX lanes.
- Factory presets bring their effects. Saving a preset saves the rack with it.
- Effects and patches are swapped on the audio thread without allocating or freeing: the old ones are handed back to the interface thread to be dropped there.

## 0.2.0-alpha.6 - 2026-10-15

The first effects, and the loudness meter.

- Effects ported from the web version: compressor (with external sidechain), look-ahead limiter, bit crusher, noise gate, transient shaper, tape, frequency shifter, pitch shifter, hall reverb, echo, step FX and the tempo-synced volume shaper. Output matches the original sample for sample on 19 reference renders.
- Loudness meter (BS.1770-4 and EBU R128): K-weighting, momentary, short-term and gated integrated loudness, loudness range, sample and true peak, stereo correlation. It reads the same numbers as the original.
- Effect settings load from the JSON the web version saved: only the keys present change.

## 0.2.0-alpha.5 - 2026-10-15

Factory presets and a preset browser.

- All 444 factory presets of the web version (Polaris 44, Vortex 36, Monolith 30, Zenith 34, TriOsc 16, Facet 33, Mist 21, Nebula 230) are included as JSON banks. Every Polaris-family, Facet and Mist preset is tested to load and play. Nebula's banks are included but Nebula itself is not ported yet.
- The app has a preset list per instrument: previous and next buttons, a search box and categories.
- Save your own presets (Save as) and delete them again. They are kept in your configuration folder, one file per instrument.

## 0.2.0-alpha.4 - 2026-10-15

Polaris, the wavetable synth, and the engine behind Vortex, Monolith, Zenith, Nebula and TriOsc.

- Wavetable synth engine ported from the web version: four wavetable oscillators with unison and ten warp modes, sub oscillator, noise, two filters in twelve types (low, high and band pass in 12 and 24 dB, notch, peak, ladder, formant, comb), four envelopes, four LFOs, a 21-source modulation matrix, mono, legato and glide. Output matches the original sample for sample on eleven reference renders covering every warp mode and filter type.
- 23 built-in band-limited, mip-mapped wavetables and the FFT behind them, checked against the original.
- `las render-synth` and `examples/synth-supersaw.json`.
- In the app: Polaris with tabs for oscillators, filters, envelopes and LFOs, a modulation matrix editor, and voice and global controls. Polaris is the default instrument.
- Wavetables and patches are prepared off the audio thread and swapped in.
- Engines share a `Playable` trait, so the command line tool and the app treat them alike.

## 0.2.0-alpha.3 - 2026-10-15

Mist, the granular synth, joins Facet.

- Mist engine ported from the TypeScript version: grains from a source sample, position, scan, spray, size, density, transpose, random pitch, stereo spread, window shape, reverse, key tracking, the modulation matrix, filter, mono and voice stealing. Output and the built-in texture match the original sample for sample on five reference renders.
- `las render-mist`, with an example patch (`examples/mist-pad.json`).
- In the app: switch between Facet and Mist, Mist controls, a Pad preset, and drop a `.wav` file on the window to use it as Mist's source. Start on Mist with `lumen --mist`.
- Engine: instruments now receive the shared random stream, so seeded renders repeat exactly.

## 0.2.0-alpha.2 - 2026-10-15

The first native app: `lumen`. It plays the Facet FM synth live.

- Native window drawn on the GPU with egui. No web view, no browser.
- Live audio output on Windows, macOS and Linux, with a lock-free link between the interface and the audio thread. Patch changes are parsed off the audio thread.
- Play with the on-screen piano, the computer keyboard (A to ; for notes, Z and X for octave) or a MIDI keyboard (notes, pitch bend, mod wheel, pressure).
- Facet controls: algorithm, feedback, four operators, amp envelope, filter, mono, legato, glide. Three starting sounds.
- Meters, DSP load, voice count and dropout counter in the window.
- Engine: patches can be prepared ahead of time and swapped in without allocating.
- Linux needs the ALSA library (`libasound2`), `libxkbcommon-x11` and OpenGL.

## 0.2.0-alpha.1 - 2026-10-15

First release of the Rust rewrite. Early: the command line tool only, no interface and no live audio yet.

- `las render-facet`: renders a Facet (four-operator FM) patch and a list of notes to a 32-bit float stereo WAV.
- Facet engine ported from the TypeScript version: eight algorithms, operator feedback, mono, legato and glide modes, voice stealing, two LFOs (including sync, sample and hold, smooth), a modulation matrix, and a per-voice filter.
- The Rust output matches the TypeScript engine sample for sample on five reference renders (see `docs/PARITY.md`).
- Two example patches in `examples/`.
- Licensed GPL-3.0-or-later.
