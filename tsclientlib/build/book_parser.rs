use std::default::Default;
use tsproto_structs::book;
use tsproto_structs::book::*;
use tsproto_util::*;

#[derive(Template)]
#[TemplatePath = "build/BookDeclarations.tt"]
#[derive(Debug)]
pub struct BookDeclarations<'a>(pub &'a book::BookDeclarations);

impl Default for BookDeclarations<'static> {
	fn default() -> Self { BookDeclarations(&tsproto_structs::book::DATA) }
}
