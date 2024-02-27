use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use bumpalo::Bump;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Loc {
    range: Range<usize>,
    line: usize,
    col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location<'bump> {
    loc: &'bump Loc,
    path: &'bump PathBuf,
}

impl<'bump> Location<'bump> {
    pub fn new_in(
        range: Range<usize>,
        line: usize,
        col: usize,
        path: &'bump PathBuf,
        bump: &'bump Bump,
    ) -> Self {
        let loc = bump.alloc_with(|| Loc { range, line, col });
        Self { loc, path }
    }

    pub fn clone_into_bump<'n>(&self, b: &'n Bump) -> Location<'n> {
        let loc = b.alloc_with(|| self.loc.clone());
        let path = b.alloc_with(|| self.path.clone());
        Location { loc, path }
    }

    pub fn range(&self) -> &'bump Range<usize> {
        &self.loc.range
    }

    pub fn start(&self) -> usize {
        self.loc.range.start
    }

    pub fn end(&self) -> usize {
        self.loc.range.end
    }

    pub fn line(&self) -> usize {
        self.loc.line
    }

    pub fn col(&self) -> usize {
        self.loc.col
    }

    pub fn path(&self) -> &'bump Path {
        self.path
    }
}
