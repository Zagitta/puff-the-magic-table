
//#![feature(proc_macro_span)]
use git2::{Repository, RepositoryOpenFlags};
use proc_macro::{Span, TokenStream};
use std::iter;

#[proc_macro_derive(MagicTable, attributes(ptmt))]
pub fn derive_serializable(item: TokenStream) -> TokenStream {
    let span = Span::call_site();
    //let source = span.source_file();

    let _ = parse();

    item
}

fn parse() -> anyhow::Result<()> {
    let repo = Repository::discover(".")?;
    Ok(())
}
