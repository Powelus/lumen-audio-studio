# Changelog

Each version that lands on `main` is published as a GitHub Release. Versions with a `-` (for example `-alpha.1`) are pre-releases.

## 0.2.0-alpha.50

CLAP plugin windows.

- A CLAP channel or effect now has the Plugin window button too, and opens the plugin's own window, as VST3 plugins have since 0.2.0-alpha.48. What you turn in it goes into the project with undo, and the playing plugin follows.
- The host side serves what CLAP windows need: size requests and closing, parameter flushes, timers, and on Linux and macOS the file descriptors that drive a plugin's event loop.
- The test CLAP plugin has a window, and a test opens it in a real X server and checks the edits it sends, including one from a timer.

## 0.2.0-alpha.51

Plugins in their own process.

- Each CLAP or VST3 plugin now runs in its own small program, `lumen-plugin-host`, which ships next to the app. A plugin that crashes takes only that program with it: its channel or effect goes silent and the app, the project and the other plugins keep playing. A plugin that hangs for two seconds is treated the same way.
- Notes, audio, parameters and state go to the plugin over a pipe, block by block. The result is the same as running it inside the app (a test checks this sample for sample with both test plugins).
- Set `LUMEN_PLUGINS_IN_PROCESS=1` to run plugins inside the app as before. The plugin windows still open inside the app.
## 0.2.0-alpha.52

Settings: your own sample and plugin folders.

- Options > Settings: add as many sample folders as you like; they show in the browser's Samples tab under My folders, as folders you can open. Click a WAV file to hear it, double click it to add it as a sampler channel. The search box searches them too. The + next to My folders adds one straight from the browser.
- Plugin folders: add folders to look for CLAP and VST3 plugins in, besides the standard ones (which can be switched off). Rescan plugins now, or the Rescan button in the Plugins tab, uses them. A folder that is not there is shown struck through.
- Autosave: a project with changes is copied to the app's autosave folder every 5 minutes (0 turns it off). Its own file is left alone.
- File > Open recent lists the last ten projects opened or saved.
- The windows that were open (browser, rack, playlist, piano roll, mixer) open again at the next start.
- A menu bar across the top (File, Edit, Add, Patterns, View, Options, Help) with a Settings button at its right replaces the menu button in the top bar.
- The app opens maximised and sizes everything to the screen: the layout fits a 1280 by 720 screen and grows on bigger ones (about 135% at 1080p, 180% at 1440p, 270% at 4K), drawn at the screen's real resolution so it stays sharp. Settings > Display sets your own size from 50% to 200% on top. `--size 1920x1080` opens the window at a given size instead.
- Settings are kept in `settings.json` in the app's folder.
- Analyser (the Analyser button in the top bar): what the master output sounds like, four ways. A heat map scrolls the spectrum by in time, low notes at the bottom and loud in bright yellow. A spectrum shows the level of each frequency with a slowly falling peak line. A goniometer draws left against right (a mono sound is an upright line, a wide one a cloud) with correlation and balance bars. Loudness shows momentary, short term and integrated LUFS, true peak, and left and right peak meters. The audio thread hands the output over without waiting or allocating.
## 0.2.0-alpha.53

Monolith, Zenith and TriOsc get their own faces.

- Monolith, Zenith and TriOsc are now instruments in their own right instead of recoloured copies of Polaris. Monolith is a one page virtual analogue laid out like a classic hardware synth between wooden side panels, with three oscillators, a mixer with noise, a big filter cutoff, two contour envelopes and an LFO. Zenith is built for supersaws and pads, with a big unison section that draws the detuned voices, an XY pad for detune and brightness, quick chorus, echo and reverb controls, a saw layer and a pad and sub layer. TriOsc is a small window with three simple oscillator rows and a compact filter and envelope strip.
- Each comes with 32 new presets of its own, and the preset list now shows the presets of the instrument you are looking at. Polaris and Vortex keep the full wavetable panel.
## 0.2.0-alpha.56

Faster start, an icon, and a tidier download.

- The app starts faster, above all on Windows: it draws with OpenGL instead of probing DirectX and Vulkan first, and it uses its own bundled fonts instead of scanning every font on the system. The programs are built without debug symbols, so they are smaller.
- Lumen Audio Studio has its own icon: a glowing L running into a sound wave. It is on the window and, on Windows, on `lumen.exe`.
- The download shows one program: `lumen` (the app). `las`, the command line renderer, is now in the `tools` folder.
- This release also carries everything of 0.2.0-alpha.50 to 0.2.0-alpha.55 below.

## 0.2.0-alpha.49

Weave, a modular synth with patch cables.

- Add it from the browser's Plugins tab (Weave). Its window is the patch: modules on a canvas joined by cables. Drag a module by its title, drag the canvas to move around, drag from an output (right) to an input or a parameter (left) to plug a cable in. Click a cable's handle to set its amount (-1 to 1) or pull it out; right click a handle to pull it out at once; right click a title to remove a module. Add module, Voices and Reset are on top. Every edit is one undo step and is saved in the project.
- Modules: keys, oscillator (sine, triangle, saw, square with polyBLEP), noise, LFO, ADSR envelope, state variable filter (lowpass, bandpass, highpass), VCA, four input mixer, math (scale and offset) and output.
- Every parameter is also an input: a cable into a parameter modulates it over its range, scaled by the cable's own amount. Loops of cables are allowed and run one sample late, and signals are kept finite.
- Each voice runs its own copy of the graph (8 voices by default, up to 32, the oldest is taken when all play). Notes start on their exact sample.
- Five factory patches (Patches menu): Saw lead, Wobble bass, Pluck, Slow pad, Noise sweep.
- Right click any knob: Create automation clip. Every parameter of every module can be automated (the id is the module id times 64 plus the parameter's place).
- Engine: `las_dsp::modular` (the synth) and `las_engine::weave` (the channel instrument), with tests for pitch, release, modulation, loops, polyphony, patches, note timing and automation; the editor's patch edits are tested too.
## 0.2.0-alpha.48

VST3 plugin windows.

- A VST3 channel or effect has a Plugin window button that opens the plugin's own window. What you turn in it goes into the project as parameter changes (with undo), and the playing plugin follows. The generic sliders and the window stay in step.
- The window is shown by a second instance of the plugin on the app's thread, so the audio thread is never touched by the plugin's UI. Plugins that talk between their window and their audio part by private messages (meters, waveforms) show only what their parameters hold.
- New `las-window` crate: a plain native window for a plugin to draw into (X11 on Linux, Win32 on Windows, NSWindow on macOS). On Linux the host also serves the plugin's file descriptors and timers (the VST3 run loop).
- macOS: VST3 plugins are now entered with their bundle (`bundleEntry`), as the specification asks.
- The test VST3 plugin has a view, and a test opens it in a real X server (Xvfb) and checks the edits it sends, including one from a run loop timer.
- Not yet: CLAP plugin windows, and running plugins in a separate process.

## 0.2.0-alpha.47

Velocity tools.

- The piano roll's Tools menu has Softer, Louder and Random velocity, on the selected notes (or all of them), each one undo step.

## 0.2.0-alpha.46

Random steps and taller tracks.

- The step sequencer's Fill menu has "Randomize": about a third of the channel's steps, picked at random (one undo step).
- The playlist has a Height slider for its tracks, next to Zoom.

## 0.2.0-alpha.45

Shift steps.

- The step sequencer's Fill menu can shift a channel's steps one step left or right; steps that fall off one end come back at the other, as in FL.

## 0.2.0-alpha.44

Fill steps.

- Right-click a channel in the step sequencer, Fill: a step on every step, or each 2, 4 or 8 steps, as FL's "Fill each N steps" does; "Clear steps" removes them. Notes drawn in the piano roll are left alone. One undo step.

## 0.2.0-alpha.43

Fit the song in the playlist.

- The playlist's "Fit" button zooms so that the whole song (and a bar after it) fits the window.
- The playlist's hint now says what a right click does (markers, loops and clip options); it still said "right click deletes".

## 0.2.0-alpha.42

Keyboard shortcuts.

- Help > Keyboard shortcuts (or F1) lists the shortcuts of the whole app, the piano roll, the playlist and the mixer.
- Ctrl+N and Ctrl+O now make a new project and open one, as the File menu said they would (asking first when there are unsaved changes, as the menu does).

## 0.2.0-alpha.41

Swing per channel.

- Each channel in the step sequencer can have its own swing: right-click the channel's name, Swing, and pick from 0 to 75%, or "Song's swing" to follow the song again. It moves only that channel's steps, in the player, renders and exports. Saved in the project; a value that is not a number from 0 to 1 is put right when the project opens. A test checks that it moves one channel's steps and not another's.

## 0.2.0-alpha.40

Make unique.

- Right-click a pattern clip in the playlist and pick "Make unique", as in FL: the clip gets its own copy of the pattern (named "... copy"), so editing it no longer changes the other clips of the original. One undo step.

## 0.2.0-alpha.39

Clip colours.

- Right-click a pattern or automation clip in the playlist for a palette of twelve colours, as in FL's playlist. The colour belongs to the pattern (or automation clip), so every clip of it changes, and it is one undo step.

## 0.2.0-alpha.38

Export the loop.

- The export window has "Only the loop": with a song loop set, it renders just that stretch of the song (with the usual tail), for bouncing a section or a loop to a sample. A test checks that a range renders what the live player plays from the range's start.

## 0.2.0-alpha.37

Song loops.

- Loop part of the song, as FL's playlist selection does: right-click the playlist and pick "Loop between these markers", "Loop this bar" or "Loop four bars from here". Play song then goes round that stretch until "Stop looping". The loop shows as a band on the ruler, and a song that is playing picks a new loop up at once. A test plays a loop in the live player and checks that the playhead comes back to its start and stays inside it.

## 0.2.0-alpha.36

Piano roll tools and song markers.

- Piano roll Tools menu, after FL's: Legato, Strum, Flip, Reverse, Humanize, Chop (sixteenths), Velocity ramp, Double and Half length, and Snap to scale (when a scale is highlighted). Each works on the selected notes, or on all of them when none are selected, and is one undo step.
- Song markers: right-click the playlist to add a marker on the snap, rename or delete the one under the pointer, or jump to any marker. Markers show as named flags on the ruler and are saved in the project.

## 0.2.0-alpha.35

FL style faders, an undo history, and the "Modern Talking" bass.

- Mixer faders in FL's style: a tall groove with a dB scale and a 0 dB line, a ridged fader cap, and a segmented meter that runs green, yellow and red. Drag to move (Shift for fine), double-click for 0 dB; the tooltip shows the level in dB. The mixer window is taller to fit them.
- Edit > History lists every step undo can reach, each named by what it did ("Add channel", "Tempo 99.0", "Edit Kick", "Delete notes", "Mixer"...), plus the steps redo can reach. A click goes straight to that step.
- "Modern Talking" (Vortex and Polaris, Bass): a talking wobble bass. A formant wavetable and a reese, a sine sub, and a formant filter, with a tempo-synced LFO sweeping both the table and the filter, into drive and compression. A test checks that it sounds and that its tone moves in quarter notes.

## 0.2.0-alpha.34

Instrument windows that look like plugins, every instrument in the browser, MP3 export, and piano roll scales and chord stamps.

- Plugin faces: each instrument window is painted like the synth it is modelled on, with its own colours, panels, knobs and a header bar with its name. Polaris and Vortex are graphite with blue screens, Monolith silver with a blue LCD, Zenith black and orange, TriOsc FL grey and green, Facet black and amber, Mist teal, Nebula night blue and magenta, LumaPad black and red. Effect windows are dark steel rack units with green LEDs.
- Wavetable screens: each oscillator shows its wavetable at the current position. Vortex shows the whole table in 3D, Serum style: every frame stacked in perspective over a floor grid, with the playing frame glowing. Mist shows its grain cloud around the play position.
- Nebula has its own window: 230 factory presets by category, master and macro knobs, and layer cards (on, name, waveform, level, pan, semitones; add and remove layers). A new Nebula starts from its Init preset.
- LumaPad has its own window: a 4 by 4 grid of pads you can click to play. Right-click a pad to give it any factory sample; each pad has level, pan, tune and a choke group. A new LumaPad comes with a sixteen pad kit.
- The browser's Plugins tab adds any instrument (Vortex, Monolith, Zenith, TriOsc, Nebula, Sampler, LumaPad included) as a channel and opens its window. Before, most were greyed out.
- Export to MP3 at 320 or 192 kbit/s. The file name follows the chosen format.
- Piano roll: highlight a scale (any root; major, minor, harmonic minor, dorian, phrygian, pentatonics), dimming the keys outside it. Chord stamps (major, minor, 7ths, sus, diminished, power, octave) make each click draw the whole chord, as FL's stamps do.
- Fixes: Polaris' and Facet's right-hand oscillators and operators were cut off (Facet's operators are now a 2 by 2 grid); the piano roll grid stopped after one bar; the mixer's insert panel ran off the window; a plugin face's background ran past the bottom of a shrunk window; a channel's window no longer shows the stand-alone synth switch.

## 0.2.0-alpha.33

A fix for new channels.

- A synth added from the browser (Polaris, Vortex, Monolith, Zenith, TriOsc, Facet, Mist, Nebula) stopped the whole project from playing with "invalid type: null, expected struct SynthPatch". A new channel has no patch yet; it now plays the instrument's default sound. A new Nebula starts with one layer instead of none (the starter project's Nebula was silent).
- A channel whose settings cannot be read is now silent instead of stopping the rest of the project from playing.
- A test adds each instrument the way the browser does and checks that it sounds, offline and in the live player.

## 0.2.0-alpha.32

Audio clips, recording, and a CPU and memory display.

- Drop a WAV file on the playlist and it becomes an audio clip where it lands (on the snap, on that track), the way FL Studio does it. Dropped anywhere else it is still a sampler channel. Audio clips show the outline of their sound.
- Recording: arm the red record button and press play. The default audio input is captured while the song plays. On stop, the take becomes an audio clip where recording started, on the first free track.
- The top bar shows the CPU and memory use, as in FL Studio: a bar graph and percentage of how much of each audio buffer's time the engine takes (red when it nears the limit), and the memory in use in MB.
- The top bar is tighter, so it all fits in a 1280 pixel wide window.

## 0.2.0-alpha.31

VST3 plugins, and automation of hosted plugins.

- VST3 hosting next to CLAP: `.vst3` bundles (and Windows single-file plugins) are found in the usual VST3 folders and in `VST3_PATH`, and run as instruments or effects with sample accurate notes, parameters (0 to 1, as VST3 has them) and the component's state, which the plugin's controller follows. The browser lists both kinds and says which is which.
- Automation clips for hosted plugins: right click a parameter's slider in a plugin's window and pick "Create automation clip". It works for a channel's plugin and for a plugin in a mixer slot, in the live player, renders and exports.
- `las-testvst3` is a small VST3 plugin made for the tests (a gain, a sine from notes, state). The tests load it as a real library: scanning, effect and instrument processing, note offsets, parameters, state, and whole projects with it as an instrument, an effect and automated. The same automation tests run on the CLAP test plugin.
- Not yet: the plugins' own windows, and running plugins in a separate process. On macOS, VST3 plugins are loaded without the bundle entry call (most plugins do not need it).

## 0.2.0-alpha.30

Plugin hosting: CLAP plugins load as instruments and as effects.

- New crate `las-host` (on the `clack` host library) finds `.clap` plugins in the usual folders and in `CLAP_PATH`, reads what each one is (instrument, effect or both) and runs them: notes in and audio out for an instrument, audio in and out for an effect, with sample accurate note timing, parameter changes and the plugin's saved state. The engine only knows a small `PluginHost` interface, so offline renders, exports and the live player all play hosted plugins the same way.
- The browser's Plugins tab lists the plugins found (with Rescan). A click on an instrument adds it as a channel; the "+ Slot" menu of a mixer insert lists the effects. The window of a channel or slot shows a slider for each parameter of the plugin; the values are saved in the project with the plugin's id (never a file path, so a project cannot make the app load an arbitrary file). A project that names a plugin that is not installed plays silent (a channel) or is skipped (an effect) instead of failing to open.
- `las-testplug` is a small CLAP plugin made for the tests (a gain parameter, a sine from note events, state). The tests load it as a real library: scanning, effect and instrument processing, note offsets, block splitting, parameters, state round trip, and whole projects rendered by the engine with it as an instrument and as an effect.
- Not yet: VST3, the plugins' own windows, plugin state beyond parameters, and running plugins in a separate process (a crashing plugin can still take the app down).

## 0.2.0-alpha.26

Selecting, moving and copying in the editors.

- Piano roll and playlist: with the Select tool, drag a rectangle to select notes or clips (Shift adds), click to select one, drag any selected one to move them all together in time and pitch (or between tracks); a group never leaves the grid. With the pointer over the editor: Delete removes the selection, Ctrl+A selects all, Ctrl+C, Ctrl+X and Ctrl+V copy, cut and paste (notes at the last place clicked, clips at the start marker), Ctrl+D duplicates the selection right after itself, and in the roll the arrows nudge in time and transpose (Shift for an octave). All of it joins undo.

## 0.2.0-alpha.29

- The playlist now runs at least 128 bars (and eight bars past the last clip) instead of ending just after the song, so there is room to arrange.
- Pattern clips in the playlist show a small preview of their notes, repeated as often as the clip is long.

## 0.2.0-alpha.28

Looking after the project the way FL Studio does.

- Right click a channel's name in the rack: piano roll, instrument, Rename, Clone (with its notes), Colour, Send to insert (route it to any mixer insert), Delete (with its notes in every pattern). Patterns > Rename pattern. Right click a playlist track's name to rename or mute it, and a light on each track mutes it (the engine already honours that). All of it joins undo.
- Includes everything of 0.2.0-alpha.26 and 0.2.0-alpha.27 below (selection and copy and paste, the sampler window, the new default look).

## 0.2.0-alpha.27

A sampler window, and a new default look.

- Double click a sampler channel (the drums of the starter project, anything imported) to open its window: the sample as a waveform with start and end markers to drag, knobs for coarse and fine tuning, attack, decay, sustain and release, velocity, filter cutoff, resonance and boost, root key, loop, reverse and "cut itself", and the filter type. A test checks that the settings it writes are the ones the engine reads.
- The app starts in the FL Studio style look (gunmetal grey with orange, the "Orange Steel" theme) instead of the violet one. The violet that projects of the web version are saved with (patterns, the master) follows whichever theme is chosen. The other seven themes are in the menu under Options.

## 0.2.0-alpha.25

More of an FL Studio workflow.

- Import your own samples: File > Import samples..., the Import button in the browser, or drop WAV files on the window. Each becomes a sampler channel with its own insert, and the file is stored in the project. Double click a factory sample in the browser to add it as a channel (a single click still plays it).
- Metronome: a button next to Stop; a click on every beat (higher on the first of the bar) while the project plays.
- Piano roll: a velocity lane under the grid; drag in it to set the velocity of the notes above.
- Mixer routing: choose what an insert plays to, and add sends with their own level and a sidechain-only switch. Loops are refused.
- Level meters on every mixer strip while the project plays.
- Automation clips, as in FL Studio: right click a fader or a knob (an insert's volume or pan, a channel's level or pan, the master's volume), or pick a parameter from the "Automate" menu of an effect's window, and a clip is made at the start marker, flat at the parameter's value. It sits in the playlist with a preview of its curve; double click it to edit the curve in its own window (click to add a point, drag to move, right click to remove, set its length in bars, rename or delete it). The engine plays them as the web version does (a test checks a made clip really pulls a fader down in the render).
- This release also carries everything of 0.2.0-alpha.24 above, which was not published on its own.

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
