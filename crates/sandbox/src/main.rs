use anyhow::{anyhow, Context, Ok};
use bumpalo::{collections::Vec, Bump};
use git2::{
    Blob, Commit, DiffLineType, DiffOptions, Oid, Pathspec, PathspecFlags, Repository, Sort, Time,
    Tree,
};
use itertools::Itertools;
use quote::ToTokens;
use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    ops::{Range, RangeInclusive},
    path::Path,
};
use syn::spanned::Spanned;

#[derive(Debug)]
enum FieldChange<'a> {
    Removed { name: &'a str },
    Added { name: &'a str, ty: &'a str },
    Renamed { from: &'a str, to: &'a str },
}

#[derive(Debug)]
struct ChangeSet<'b, 'a> {
    revision: Oid,
    time: Time,
    data: Vec<'b, FieldChange<'a>>,
}

#[derive(Debug)]
struct TrackedModel<'a> {
    name: Cow<'a, String>,
    extent: RangeInclusive<usize>,
}

#[derive(Debug)]
struct ChangeColletion<'b, 'a> {
    change_sets: Vec<'b, ChangeSet<'b, 'a>>,
}

impl<'b, 'a> ChangeColletion<'b, 'a> {
    pub fn new(bump: &'b Bump) -> Self {
        ChangeColletion {
            change_sets: Vec::new_in(bump),
        }
    }
}

fn foobar() {
    let b = Bump::new();
    let cc = ChangeColletion::new(&b);
}

fn match_with_parent(
    repo: &Repository,
    commit_tree: &Tree,
    parent_tree: &Tree,
    opts: &mut DiffOptions,
) -> anyhow::Result<bool> {
    let diff = repo.diff_tree_to_tree(Some(parent_tree), Some(commit_tree), Some(opts))?;
    Ok(diff.deltas().len() > 0)
}

fn tracking<'a>(
    repo: &'a Repository,
    path: &'a Path,
    mut start: usize,
    mut end: usize,
) -> anyhow::Result<()> {
    let ps = Pathspec::new(&[&path])?;
    let mut diff_opts = DiffOptions::new();
    diff_opts.pathspec(&path);

    let mut revwalk = repo.revwalk()?;

    revwalk.set_sorting(Sort::TIME)?;
    revwalk.push_head()?;

    let mut count = 0;

    let mut commits = revwalk.inspect(|_| count += 1).filter_map(move |rev| {
        let oid = rev.unwrap();
        let commit = repo.find_commit(oid).unwrap();
        let tree = commit.tree().unwrap();

        match commit.parent_count() {
            0 => ps
                .match_tree(&tree, PathspecFlags::NO_MATCH_ERROR)
                .ok()
                .map(|_| commit),
            _ => commit
                .parents()
                .all(|parent| {
                    match_with_parent(&repo, &tree, &parent.tree().unwrap(), &mut diff_opts)
                        .unwrap_or(false)
                })
                .then_some(commit),
        }
    });

    let mut changes = HashMap::new();

    let prev = commits.next().ok_or(anyhow!("rel sad"))?;
    let mut prev_blob = extract_blob(repo, &prev, path)?;

    let mut diff_opts = DiffOptions::new();
    diff_opts.pathspec(path);
    diff_opts.context_lines(0);

    for curr in commits {
        let curr_blob = extract_blob(repo, &curr, path)?;

        let mut start_move = start;
        let mut end_move = end;

        let mut foo = vec![];
        repo.diff_blobs(
            Some(&prev_blob),
            None,
            Some(&curr_blob),
            None,
            Some(&mut diff_opts),
            None,
            None,
            None,
            Some(&mut |_d, _h, l| {
                let content_offset = l.content_offset() as usize;
                let len = l.content().len();
                foo.push((l.origin_value(), l.old_lineno().or(l.new_lineno())));
                match l.origin_value() {
                    DiffLineType::Addition => {
                        if start > content_offset {
                            start_move += len;
                        }
                        if end > content_offset {
                            end_move += len;
                        }
                    }
                    DiffLineType::Deletion => {
                        if start > content_offset {
                            start_move -= len;
                        }
                        if end > content_offset {
                            end_move -= len;
                        }
                    }
                    _ => {}
                };
                content_offset <= end
            }),
        )?;

        println!("changes: {:#?}", foo);

        start = start_move;
        end = end_move;

        if end <= start {
            break;
        }

        let curr_content = std::str::from_utf8(curr_blob.content())?;

        let snippet = &curr_content[start as usize..end as usize];
        let s = syn::parse_str::<syn::ItemStruct>(snippet).with_context(|| snippet.to_string())?;

        /* match &s.fields {
            syn::Fields::Named(n) => {
                let f = n.named.first().unwrap();
                let s = f.span();

                println!("'{}' span: {:?}", f.ident.as_ref().unwrap(), s);
            }
            _ => {}
        } */

        /* let decl = venial::parse_declaration(snippet.parse().unwrap()).unwrap();
        let s = decl.as_struct().ok_or(anyhow!("more sad"))?;

        match &s.fields {
            venial::StructFields::Unit => todo!(),
            venial::StructFields::Tuple(_) => todo!(),
            venial::StructFields::Named(n) => {
                let f = n.fields.iter().next().unwrap();
                let s = f.0.span();
                println!("'{}' span: {:?}", f.0.name, s);
            }
        } */

        changes.insert(
            s.fields.to_token_stream().to_string(),
            (curr.id(), curr.time()),
        );

        prev_blob = curr_blob;
    }

    let changes = changes
        .into_iter()
        .sorted_unstable_by_key(|(_, (_, time))| time.seconds())
        .map(|(k, _)| k)
        .collect_vec();

    println!("{:#?}", changes);
    println!("processed {count} commits");

    Ok(())
}

fn extract_blob<'a, 'b, 'c>(
    repo: &'a Repository,
    commit: &'b Commit,
    path: &'c Path,
) -> Result<Blob<'a>, anyhow::Error> {
    let tree = commit.tree()?;
    let entry = tree.get_path(path)?;
    let object = entry.to_object(repo)?;
    let blob = object.into_blob().map_err(|e| anyhow!("sad"))?;
    Ok(blob)
}

fn find_start_end(content: &str, needle: &str) -> Option<(usize, usize)> {
    let start = content.find(needle)?;
    let end = start + content[start..].find('}')? + 1;
    //adjust end to nearest preceding newline
    let start = content[..start + 1].rfind('\n').unwrap_or(start);
    Some((start, end))
}

impl<'a> TrackedModel<'a> {
    pub fn from_content(name: Cow<'a, String>, content: &str) -> Option<Self> {
        let start = content.find(name.as_str())?;
        let end = start + content[start..].find('}')? + 1;
        //adjust end to nearest preceding newline
        let start = content[..start + 1].rfind('\n').unwrap_or(start);

        Some(Self {
            name,
            extent: start..=end,
        })
    }

    pub fn gather_revisions(&self, repo: &Repository) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let curr_dir = env::current_dir()?;
    let repo = Repository::discover(&curr_dir.join("repos/basic"))?;
    let file_path = Path::new("basic.rs");

    let mut diff_opts = DiffOptions::new();
    diff_opts.pathspec(file_path);
    diff_opts.context_lines(0);

    let diff = repo.diff_index_to_workdir(None, Some(&mut diff_opts))?;
    diff.print(git2::DiffFormat::Raw, |d, h, l| {
        println!("{:#?}", l);
        true
    })?;

    let head = repo.head()?;
    let head = head.peel_to_commit()?;
    let blob = extract_blob(&repo, &head, file_path)?;
    let content = std::str::from_utf8(blob.content())?;
    let (start, end) =
        find_start_end(&content, "struct Foobar").ok_or(anyhow!("Failed to find struct"))?;

    let start_time = std::time::Instant::now();

    tracking(&repo, file_path, start, end)?;

    let end = std::time::Instant::now();

    println!("Took: {}ms", (end - start_time).as_millis());

    Ok(())
}
