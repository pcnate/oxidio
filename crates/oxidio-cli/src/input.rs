//! Input mode handling for the TUI.
//!
//! Manages the current input mode (Normal, Command, Search) and
//! provides an input buffer for text entry.


/// Current input mode of the application.
#[derive( Debug, Clone, Copy, PartialEq, Eq, Default )]
pub enum InputMode {
    /// Normal mode - keyboard shortcuts active.
    #[default]
    Normal,

    /// Command mode - typing a slash command.
    Command,

    /// Search/filter mode - typing search term.
    Search,
}


/// Input buffer for command/search text entry.
#[derive( Debug, Default )]
pub struct InputBuffer {
    content: String,
    cursor: usize,
}


impl InputBuffer {
    /// Creates a new empty input buffer.
    pub fn new() -> Self {
        Self::default()
    }


    /// Inserts a character at the cursor position.
    pub fn insert( &mut self, c: char ) {
        self.content.insert( self.cursor, c );
        self.cursor += c.len_utf8();
    }


    /// Deletes the character before the cursor.
    pub fn backspace( &mut self ) {
        if self.cursor > 0 {
            let prev_char_boundary = self.content[ ..self.cursor ]
                .char_indices()
                .last()
                .map( |( i, _ )| i )
                .unwrap_or( 0 );
            self.content.remove( prev_char_boundary );
            self.cursor = prev_char_boundary;
        }
    }


    /// Deletes the character at the cursor position.
    pub fn delete( &mut self ) {
        if self.cursor < self.content.len() {
            self.content.remove( self.cursor );
        }
    }


    /// Clears the buffer.
    pub fn clear( &mut self ) {
        self.content.clear();
        self.cursor = 0;
    }


    /// Gets the current content.
    pub fn content( &self ) -> &str {
        &self.content
    }


    /// Gets the cursor position (byte offset).
    pub fn cursor( &self ) -> usize {
        self.cursor
    }


    /// Gets the cursor position as character count (for display).
    pub fn cursor_char_pos( &self ) -> usize {
        self.content[ ..self.cursor ].chars().count()
    }


    /// Moves cursor left by one character.
    pub fn move_left( &mut self ) {
        if self.cursor > 0 {
            self.cursor = self.content[ ..self.cursor ]
                .char_indices()
                .last()
                .map( |( i, _ )| i )
                .unwrap_or( 0 );
        }
    }


    /// Moves cursor right by one character.
    pub fn move_right( &mut self ) {
        if self.cursor < self.content.len() {
            self.cursor = self.content[ self.cursor.. ]
                .char_indices()
                .nth( 1 )
                .map( |( i, _ )| self.cursor + i )
                .unwrap_or( self.content.len() );
        }
    }


    /// Moves cursor to the beginning.
    pub fn move_home( &mut self ) {
        self.cursor = 0;
    }


    /// Moves cursor to the end.
    pub fn move_end( &mut self ) {
        self.cursor = self.content.len();
    }


    /// Returns true if the buffer is empty.
    pub fn is_empty( &self ) -> bool {
        self.content.is_empty()
    }
}
