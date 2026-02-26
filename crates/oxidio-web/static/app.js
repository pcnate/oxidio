/**
 * Oxidio Web UI — WebSocket client and UI controller.
 */
( function() {
    "use strict";

    // --- State ---

    let ws = null;
    let reconnectTimer = null;
    let state = {
        playback_state: "stopped",
        current_track: null,
        position_secs: 0,
        duration_secs: null,
        volume: 1.0,
        playlist: [],
        playlist_index: null,
        shuffle: false,
        repeat_mode: "off",
        view_mode: "playlist",
        browser: null,
        settings: {
            discord_enabled: false,
            smtc_enabled: false,
            web_enabled: true,
            web_port: 8384,
            web_bind: "127.0.0.1",
        },
    };
    let currentView = "playlist";
    let statusTimeout = null;


    // --- WebSocket ---


    /**
     * Connects to the WebSocket server with auto-reconnect.
     */
    function connect() {
        const proto = location.protocol === "https:" ? "wss:" : "ws:";
        const url = `${proto}//${location.host}/ws`;

        ws = new WebSocket( url );

        ws.onopen = function() {
            setConnectionStatus( true );
            clearTimeout( reconnectTimer );
        };

        ws.onclose = function() {
            setConnectionStatus( false );
            scheduleReconnect();
        };

        ws.onerror = function() {
            ws.close();
        };

        ws.onmessage = function( event ) {
            try {
                const msg = JSON.parse( event.data );
                handleMessage( msg );
            } catch ( e ) {
                console.error( "Failed to parse message:", e );
            }
        };
    }


    /**
     * Schedules a reconnection attempt.
     */
    function scheduleReconnect() {
        clearTimeout( reconnectTimer );
        reconnectTimer = setTimeout( connect, 2000 );
    }


    /**
     * Sends a command to the server.
     *
     * @param cmd - The command object to send
     */
    function sendCommand( cmd ) {
        if ( ws && ws.readyState === WebSocket.OPEN ) {
            ws.send( JSON.stringify( cmd ) );
        }
    }


    // --- Message handling ---


    /**
     * Handles an incoming state update message.
     *
     * @param msg - The parsed message object
     */
    function handleMessage( msg ) {
        switch ( msg.type ) {
            case "full_state":
                // FullState has fields flattened (serde flatten)
                Object.assign( state, msg );
                delete state.type;
                renderAll();
                break;

            case "position":
                state.position_secs = msg.secs;
                renderProgress();
                break;

            case "playback_state_changed":
                state.playback_state = msg.state;
                renderPlayButton();
                break;

            case "track_changed":
                state.current_track = msg.track;
                state.duration_secs = msg.duration_secs;
                state.position_secs = 0;
                renderNowPlaying();
                renderPlaylist();
                break;

            case "playlist_changed":
                state.playlist = msg.playlist;
                state.playlist_index = msg.index;
                renderPlaylist();
                break;

            case "volume_changed":
                state.volume = msg.level;
                renderVolume();
                break;

            case "mode_changed":
                state.shuffle = msg.shuffle;
                state.repeat_mode = msg.repeat_mode;
                renderModes();
                break;

            case "settings_changed":
                state.settings = msg.settings;
                renderSettings();
                break;

            case "browser_changed":
                state.browser = msg.browser;
                renderBrowser();
                break;

            case "status_message":
                showStatus( msg.message );
                break;
        }
    }


    // --- Rendering ---


    /**
     * Re-renders the entire UI from state.
     */
    function renderAll() {
        renderPlaylist();
        renderBrowser();
        renderNowPlaying();
        renderProgress();
        renderVolume();
        renderPlayButton();
        renderModes();
        renderSettings();
        renderTrackInfo();
    }


    /**
     * Renders the playlist track list.
     */
    function renderPlaylist() {
        const list = document.getElementById( "track-list" );
        list.innerHTML = "";

        state.playlist.forEach( function( track ) {
            const li = document.createElement( "li" );
            if ( track.index === state.playlist_index ) {
                li.classList.add( "playing" );
            }

            const idx = document.createElement( "span" );
            idx.className = "index";
            idx.textContent = ( track.index + 1 ).toString();

            const indicator = document.createElement( "span" );
            indicator.className = "indicator";
            indicator.textContent = track.index === state.playlist_index ? "\u25B6" : " ";

            const name = document.createElement( "span" );
            name.className = "name";
            name.textContent = track.display_name;

            li.appendChild( idx );
            li.appendChild( indicator );
            li.appendChild( name );

            li.addEventListener( "dblclick", function() {
                sendCommand( { cmd: "play_track", index: track.index } );
            });

            list.appendChild( li );
        });
    }


    /**
     * Renders the file browser.
     */
    function renderBrowser() {
        if ( !state.browser ) return;

        document.getElementById( "browser-path" ).value = state.browser.current_dir;

        const list = document.getElementById( "browser-list" );
        list.innerHTML = "";

        state.browser.entries.forEach( function( entry, idx ) {
            const li = document.createElement( "li" );
            if ( entry.is_dir ) li.classList.add( "dir" );
            if ( entry.is_audio ) li.classList.add( "audio" );

            const icon = document.createElement( "span" );
            icon.className = "icon";
            icon.textContent = entry.is_dir ? "\uD83D\uDCC1" : ( entry.is_audio ? "\u266B" : "\uD83D\uDCC4" );

            const name = document.createElement( "span" );
            name.textContent = entry.name;

            li.appendChild( icon );
            li.appendChild( name );

            li.addEventListener( "click", function() {
                if ( entry.is_dir ) {
                    sendCommand( { cmd: "browse_open", index: idx } );
                }
            });

            li.addEventListener( "dblclick", function() {
                if ( !entry.is_dir ) {
                    sendCommand( { cmd: "browse_add_to_playlist", index: idx } );
                }
            });

            list.appendChild( li );
        });
    }


    /**
     * Renders now-playing track info in the bar.
     */
    function renderNowPlaying() {
        const titleEl = document.getElementById( "np-title" );
        const artistEl = document.getElementById( "np-artist" );
        const durationEl = document.getElementById( "np-duration" );

        if ( state.current_track ) {
            titleEl.textContent = state.current_track.title || state.current_track.path.split( /[/\\]/ ).pop();
            artistEl.textContent = state.current_track.artist || "";
            durationEl.textContent = formatTime( state.duration_secs );
        } else {
            titleEl.textContent = "No track loaded";
            artistEl.textContent = "";
            durationEl.textContent = "0:00";
        }

        renderPlayButton();
        renderProgress();
    }


    /**
     * Renders the progress bar and position.
     */
    function renderProgress() {
        const posEl = document.getElementById( "np-position" );
        const fillEl = document.getElementById( "np-progress-fill" );

        posEl.textContent = formatTime( state.position_secs );

        const pct = state.duration_secs > 0
            ? ( state.position_secs / state.duration_secs * 100 )
            : 0;
        fillEl.style.width = pct + "%";
    }


    /**
     * Renders the play/pause button state.
     */
    function renderPlayButton() {
        const btn = document.getElementById( "btn-play" );
        btn.innerHTML = state.playback_state === "playing" ? "&#10074;&#10074;" : "&#9654;";
    }


    /**
     * Renders volume display.
     */
    function renderVolume() {
        document.getElementById( "vol-label" ).textContent =
            Math.round( state.volume * 100 ) + "%";
    }


    /**
     * Renders shuffle/repeat mode indicators on buttons and status badges.
     */
    function renderModes() {
        // Update shuffle button
        var shuffleBtn = document.getElementById( "btn-shuffle" );
        shuffleBtn.classList.toggle( "active", state.shuffle );
        shuffleBtn.classList.toggle( "shuffle-on", state.shuffle );

        // Update repeat button
        var repeatBtn = document.getElementById( "btn-repeat" );
        repeatBtn.classList.remove( "active", "repeat-one", "repeat-all" );
        if ( state.repeat_mode === "one" ) {
            repeatBtn.classList.add( "active", "repeat-one" );
            repeatBtn.title = "Repeat: One";
        } else if ( state.repeat_mode === "all" ) {
            repeatBtn.classList.add( "active", "repeat-all" );
            repeatBtn.title = "Repeat: All";
        } else {
            repeatBtn.title = "Repeat: Off";
        }

        // Update status bar badges
        var container = document.getElementById( "mode-badges" );
        container.innerHTML = "";

        if ( state.shuffle ) {
            var badge = document.createElement( "span" );
            badge.className = "badge shuffle";
            badge.textContent = "SHUFFLE";
            container.appendChild( badge );
        }

        if ( state.repeat_mode !== "off" ) {
            var badge = document.createElement( "span" );
            badge.className = "badge repeat";
            badge.textContent = state.repeat_mode === "one" ? "REPEAT 1" : "REPEAT ALL";
            container.appendChild( badge );
        }
    }


    /**
     * Renders the settings list.
     *
     * Note: web_enabled toggle is intentionally omitted (web clients cannot toggle web on/off).
     */
    function renderSettings() {
        const list = document.getElementById( "settings-list" );
        list.innerHTML = "";

        const items = [
            { key: "discord_enabled", label: "Discord Rich Presence", enabled: state.settings.discord_enabled, toggleable: true },
            { key: "smtc_enabled", label: "System Media Controls (SMTC)", enabled: state.settings.smtc_enabled, toggleable: true },
            { key: null, label: "Web Interface (port " + state.settings.web_port + ", restart to apply changes)", enabled: state.settings.web_enabled, toggleable: false },
        ];

        items.forEach( function( item ) {
            const li = document.createElement( "li" );
            if ( !item.toggleable ) li.classList.add( "locked" );

            const checkbox = document.createElement( "span" );
            checkbox.className = "checkbox";
            checkbox.textContent = item.enabled ? "[x]" : "[ ]";

            const label = document.createElement( "span" );
            label.textContent = item.label;

            li.appendChild( checkbox );
            li.appendChild( label );

            if ( item.toggleable ) {
                li.addEventListener( "click", function() {
                    sendCommand( { cmd: "toggle_setting", key: item.key } );
                });
            }

            list.appendChild( li );
        });
    }


    /**
     * Renders the track info view.
     */
    function renderTrackInfo() {
        const grid = document.getElementById( "track-info-grid" );
        grid.innerHTML = "";

        if ( !state.current_track ) {
            grid.innerHTML = '<span class="label">No track playing</span><span class="value"></span>';
            return;
        }

        const t = state.current_track;
        const fields = [
            [ "Title", t.title ],
            [ "Artist", t.artist ],
            [ "Album", t.album ],
            [ "Album Artist", t.album_artist ],
            [ "Track #", t.track_number ],
            [ "Genre", t.genre ],
            [ "Year", t.year ],
            [ "Codec", t.codec ],
            [ "Bitrate", t.bitrate ? t.bitrate + " kbps" : null ],
            [ "Sample Rate", t.sample_rate ? t.sample_rate + " Hz" : null ],
            [ "Channels", t.channels ],
            [ "Duration", t.duration_secs ? formatTime( t.duration_secs ) : null ],
            [ "Path", t.path ],
        ];

        fields.forEach( function( pair ) {
            if ( pair[1] != null ) {
                const label = document.createElement( "span" );
                label.className = "label";
                label.textContent = pair[0];

                const value = document.createElement( "span" );
                value.className = "value";
                value.textContent = String( pair[1] );

                grid.appendChild( label );
                grid.appendChild( value );
            }
        });
    }


    // --- Command Bar ---


    /**
     * Parses a time string like "1:30" or "90" into seconds.
     *
     * @param s - Time string in "M:SS" or seconds format
     *
     * @returns Number of seconds, or NaN on invalid input
     */
    function parseTime( s ) {
        s = s.trim();
        var colonIdx = s.indexOf( ":" );
        if ( colonIdx >= 0 ) {
            var min = parseInt( s.substring( 0, colonIdx ), 10 );
            var sec = parseInt( s.substring( colonIdx + 1 ), 10 );
            if ( isNaN( min ) || isNaN( sec ) ) return NaN;
            return min * 60 + sec;
        }
        return parseInt( s, 10 );
    }


    /**
     * Parses and executes a slash command string.
     *
     * @param input - The raw command text (without leading slash)
     */
    function executeCommand( input ) {
        input = input.trim();
        if ( !input ) return;

        var spaceIdx = input.indexOf( " " );
        var cmd = spaceIdx >= 0 ? input.substring( 0, spaceIdx ).toLowerCase() : input.toLowerCase();
        var args = spaceIdx >= 0 ? input.substring( spaceIdx + 1 ).trim() : null;

        switch ( cmd ) {
            // Playlist commands
            case "add":
            case "a":
                if ( !args ) { showStatus( "Usage: /add <path>" ); return; }
                sendCommand( { cmd: "add_path", path: args } );
                break;

            case "clear":
            case "cl":
                sendCommand( { cmd: "clear_playlist" } );
                break;

            case "dedup":
            case "dedupe":
            case "unique":
                sendCommand( { cmd: "dedup" } );
                break;

            case "playlist":
            case "pl":
                if ( !args ) { showStatus( "Usage: /playlist list|save|load|delete <name>" ); return; }
                var plSpaceIdx = args.indexOf( " " );
                var plSub = plSpaceIdx >= 0 ? args.substring( 0, plSpaceIdx ).toLowerCase() : args.toLowerCase();
                var plArg = plSpaceIdx >= 0 ? args.substring( plSpaceIdx + 1 ).trim() : null;

                switch ( plSub ) {
                    case "list":
                    case "ls":
                        sendCommand( { cmd: "list_playlists" } );
                        break;
                    case "save":
                        if ( !plArg ) { showStatus( "Usage: /playlist save <name>" ); return; }
                        sendCommand( { cmd: "save_playlist", name: plArg } );
                        break;
                    case "load":
                        if ( !plArg ) { showStatus( "Usage: /playlist load <name>" ); return; }
                        sendCommand( { cmd: "load_playlist", name: plArg } );
                        break;
                    case "delete":
                    case "del":
                    case "rm":
                        if ( !plArg ) { showStatus( "Usage: /playlist delete <name>" ); return; }
                        sendCommand( { cmd: "delete_playlist", name: plArg } );
                        break;
                    default:
                        showStatus( "Unknown playlist subcommand: " + plSub );
                        break;
                }
                break;

            case "shuffle":
            case "sh":
                sendCommand( { cmd: "toggle_shuffle" } );
                break;

            case "repeat":
            case "rep":
                if ( args ) {
                    var mode = args.toLowerCase();
                    if ( mode === "off" || mode === "0" ) {
                        sendCommand( { cmd: "set_repeat", mode: "off" } );
                    } else if ( mode === "one" || mode === "1" ) {
                        sendCommand( { cmd: "set_repeat", mode: "one" } );
                    } else if ( mode === "all" || mode === "2" ) {
                        sendCommand( { cmd: "set_repeat", mode: "all" } );
                    } else {
                        showStatus( "Invalid repeat mode. Use: off, one, all" );
                    }
                } else {
                    sendCommand( { cmd: "cycle_repeat" } );
                }
                break;

            // Navigation commands
            case "goto":
            case "go":
            case "cd":
                if ( !args ) { showStatus( "Usage: /goto <path>" ); return; }
                sendCommand( { cmd: "browse_to", path: args } );
                switchView( "browser" );
                break;

            case "home":
            case "~":
                sendCommand( { cmd: "browse_home" } );
                switchView( "browser" );
                break;

            // Playback commands
            case "play":
            case "p":
                sendCommand( { cmd: "play" } );
                break;

            case "pause":
            case "pa":
                sendCommand( { cmd: "pause" } );
                break;

            case "stop":
            case "st":
                sendCommand( { cmd: "stop" } );
                break;

            case "next":
            case "n":
                sendCommand( { cmd: "next" } );
                break;

            case "prev":
            case "previous":
            case "pr":
                sendCommand( { cmd: "previous" } );
                break;

            case "seek":
            case "sk":
                if ( !args ) { showStatus( "Usage: /seek <time> (e.g. 1:30 or 90)" ); return; }
                var secs = parseTime( args );
                if ( isNaN( secs ) ) { showStatus( "Invalid time format" ); return; }
                sendCommand( { cmd: "seek", position_secs: secs } );
                break;

            // UI commands
            case "vol":
            case "volume":
                if ( args ) {
                    var level = parseInt( args, 10 );
                    if ( isNaN( level ) || level < 0 || level > 100 ) {
                        showStatus( "Volume must be 0-100" );
                        return;
                    }
                    sendCommand( { cmd: "set_volume", level: level / 100.0 } );
                } else {
                    showStatus( "Volume: " + Math.round( state.volume * 100 ) + "%" );
                }
                break;

            case "help":
            case "h":
                switchView( "help" );
                break;

            case "quit":
            case "q":
            case "exit":
                sendCommand( { cmd: "quit" } );
                break;

            default:
                showStatus( "Unknown command: /" + cmd );
                break;
        }
    }


    /**
     * Opens the command bar and focuses the input.
     */
    function openCommandBar() {
        var bar = document.getElementById( "command-bar" );
        var input = document.getElementById( "cmd-input" );
        bar.classList.remove( "hidden" );
        input.value = "";
        input.focus();
    }


    /**
     * Closes the command bar and clears the input.
     */
    function closeCommandBar() {
        var bar = document.getElementById( "command-bar" );
        var input = document.getElementById( "cmd-input" );
        bar.classList.add( "hidden" );
        input.value = "";
        input.blur();
    }


    // --- UI Helpers ---


    /**
     * Formats seconds into M:SS or H:MM:SS.
     *
     * @param secs - Time in seconds
     *
     * @returns Formatted time string
     */
    function formatTime( secs ) {
        if ( secs == null || isNaN( secs ) ) return "0:00";
        secs = Math.floor( secs );
        const h = Math.floor( secs / 3600 );
        const m = Math.floor( ( secs % 3600 ) / 60 );
        const s = secs % 60;
        if ( h > 0 ) {
            return h + ":" + String( m ).padStart( 2, "0" ) + ":" + String( s ).padStart( 2, "0" );
        }
        return m + ":" + String( s ).padStart( 2, "0" );
    }


    /**
     * Shows a temporary status message.
     *
     * @param msg - The message text
     */
    function showStatus( msg ) {
        const el = document.getElementById( "status-msg" );
        el.textContent = msg;
        clearTimeout( statusTimeout );
        statusTimeout = setTimeout( function() {
            el.textContent = "";
        }, 3000 );
    }


    /**
     * Updates the connection status indicator.
     *
     * @param connected - Whether connected
     */
    function setConnectionStatus( connected ) {
        const el = document.getElementById( "conn-status" );
        if ( connected ) {
            el.textContent = "Connected";
            el.classList.remove( "disconnected" );
        } else {
            el.textContent = "Disconnected";
            el.classList.add( "disconnected" );
        }
    }


    /**
     * Switches the active view tab.
     *
     * @param viewName - The view to switch to
     */
    function switchView( viewName ) {
        currentView = viewName;

        // Update tab buttons
        document.querySelectorAll( "#header .tabs button" ).forEach( function( btn ) {
            btn.classList.toggle( "active", btn.dataset.view === viewName );
        });

        // Update view panels
        document.querySelectorAll( ".view" ).forEach( function( v ) {
            v.classList.remove( "active" );
        });

        const panel = document.getElementById( viewName + "-view" );
        if ( panel ) {
            panel.classList.add( "active" );
        }

        // Re-render view-specific content
        if ( viewName === "trackinfo" ) renderTrackInfo();
    }


    // --- Event Listeners ---


    /**
     * Initializes all event listeners.
     */
    function initEvents() {
        // Tab buttons
        document.querySelectorAll( "#header .tabs button" ).forEach( function( btn ) {
            btn.addEventListener( "click", function() {
                switchView( btn.dataset.view );
            });
        });

        // Playback controls
        document.getElementById( "btn-play" ).addEventListener( "click", function() {
            sendCommand( { cmd: "toggle_playback" } );
        });

        document.getElementById( "btn-stop" ).addEventListener( "click", function() {
            sendCommand( { cmd: "stop" } );
        });

        document.getElementById( "btn-prev" ).addEventListener( "click", function() {
            sendCommand( { cmd: "previous" } );
        });

        document.getElementById( "btn-next" ).addEventListener( "click", function() {
            sendCommand( { cmd: "next" } );
        });

        // Shuffle / Repeat toggle buttons
        document.getElementById( "btn-shuffle" ).addEventListener( "click", function() {
            sendCommand( { cmd: "toggle_shuffle" } );
        });

        document.getElementById( "btn-repeat" ).addEventListener( "click", function() {
            sendCommand( { cmd: "cycle_repeat" } );
        });

        // Browser path input — Enter to navigate
        document.getElementById( "browser-path" ).addEventListener( "keydown", function( e ) {
            if ( e.key === "Enter" ) {
                e.preventDefault();
                var path = this.value.trim();
                if ( path ) {
                    sendCommand( { cmd: "browse_to", path: path } );
                }
                this.blur();
            } else if ( e.key === "Escape" ) {
                e.preventDefault();
                // Revert to current browser path
                if ( state.browser ) {
                    this.value = state.browser.current_dir;
                }
                this.blur();
            }
        });

        // Volume
        document.getElementById( "btn-vol-down" ).addEventListener( "click", function() {
            sendCommand( { cmd: "volume_down" } );
        });

        document.getElementById( "btn-vol-up" ).addEventListener( "click", function() {
            sendCommand( { cmd: "volume_up" } );
        });

        // Progress bar seek
        document.getElementById( "np-progress-bar" ).addEventListener( "click", function( e ) {
            if ( !state.duration_secs ) return;
            const rect = this.getBoundingClientRect();
            const pct = ( e.clientX - rect.left ) / rect.width;
            const position_secs = pct * state.duration_secs;
            sendCommand( { cmd: "seek", position_secs: position_secs } );
        });

        // Command bar input handlers
        var cmdInput = document.getElementById( "cmd-input" );

        cmdInput.addEventListener( "keydown", function( e ) {
            if ( e.key === "Enter" ) {
                e.preventDefault();
                var value = cmdInput.value.trim();
                closeCommandBar();
                if ( value ) {
                    executeCommand( value );
                }
            } else if ( e.key === "Escape" ) {
                e.preventDefault();
                closeCommandBar();
            }
        });

        // Keyboard shortcuts
        document.addEventListener( "keydown", function( e ) {
            // Skip if typing in an input
            if ( e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA" ) return;

            switch ( e.key ) {
                case "/":
                    e.preventDefault();
                    openCommandBar();
                    break;
                case " ":
                    e.preventDefault();
                    sendCommand( { cmd: "toggle_playback" } );
                    break;
                case "ArrowLeft":
                    sendCommand( { cmd: "previous" } );
                    break;
                case "ArrowRight":
                    sendCommand( { cmd: "next" } );
                    break;
                case "+":
                case "=":
                    sendCommand( { cmd: "volume_up" } );
                    break;
                case "-":
                    sendCommand( { cmd: "volume_down" } );
                    break;
                case "s":
                    sendCommand( { cmd: "toggle_shuffle" } );
                    break;
                case "r":
                    sendCommand( { cmd: "cycle_repeat" } );
                    break;
                case "Tab":
                    e.preventDefault();
                    cycleView();
                    break;
            }
        });
    }


    /**
     * Cycles through views (Tab key).
     */
    function cycleView() {
        const views = [ "playlist", "browser", "trackinfo", "settings", "help" ];
        const idx = views.indexOf( currentView );
        const next = views[ ( idx + 1 ) % views.length ];
        switchView( next );
    }


    // --- Init ---

    initEvents();
    connect();

})();
