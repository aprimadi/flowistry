use crate::config::{Config, CONFIG};
use anyhow::{Error, Result};
use rustc_hir::{BodyId, ForeignItem, ImplItem, ImplItemKind, Item, ItemKind, TraitItem, itemlikevisit::ItemLikeVisitor};
use rustc_middle::ty::TyCtxt;
use rustc_span::{FileName, RealFileName, Span};

pub use intraprocedural::SliceOutput;

mod intraprocedural;
mod points_to;
mod relevance;

struct SliceVisitor<'tcx> {
  tcx: TyCtxt<'tcx>,
  slice_span: Span,
  output: Result<SliceOutput>,
}

impl<'tcx> SliceVisitor<'tcx> {
  fn analyze(&mut self, body_span: Span, body_id: &BodyId) {
    if !body_span.contains(self.slice_span) {
      return;
    }

    let tcx = self.tcx;
    let slice_span = self.slice_span;
    take_mut::take(&mut self.output, move |output| {
      output.and_then(move |mut output| {
        let fn_output = intraprocedural::analyze_function(tcx, body_id, slice_span)?;
        output.merge(fn_output);
        Ok(output)
      })
    });
  }
}

impl<'hir, 'tcx> ItemLikeVisitor<'hir> for SliceVisitor<'tcx> {
  fn visit_item(&mut self, item: &'hir Item<'hir>) {
    match &item.kind {
      ItemKind::Fn(_, _, body_id) => {
        self.analyze(item.span, body_id);
      }
      _ => {}
    }
  }
  
  fn visit_impl_item(&mut self, impl_item: &'hir ImplItem<'hir>) {
    match &impl_item.kind {
      ImplItemKind::Fn(_, body_id) => {
        self.analyze(impl_item.span, body_id);
      }
      _ => {}
    }
  }

  fn visit_trait_item(&mut self, _trait_item: &'hir TraitItem<'hir>) {}
  fn visit_foreign_item(&mut self, _foreign_item: &'hir ForeignItem<'hir>) {}
}

struct Callbacks {
  config: Option<Config>,
  output: Option<Result<SliceOutput>>,
}

impl rustc_driver::Callbacks for Callbacks {
  fn after_analysis<'tcx>(
    &mut self,
    _compiler: &rustc_interface::interface::Compiler,
    queries: &'tcx rustc_interface::Queries<'tcx>,
  ) -> rustc_driver::Compilation {
    queries.global_ctxt().unwrap().take().enter(|tcx| {
      let config = self.config.take().unwrap();

      let slice_span = {
        let source_map = tcx.sess.source_map();
        let files = source_map.files();
        let source_file = files
          .iter()
          .find(|file| {
            if let FileName::Real(RealFileName::Named(other_path)) = &file.name {
              config.path == other_path.to_string_lossy()
            } else {
              false
            }
          })
          .expect(&format!("Could not find file {}", config.path));
        config.range.to_span(source_file)
      };

      let mut slice_visitor = SliceVisitor {
        tcx,
        slice_span,
        output: Ok(SliceOutput::new()),
      };
      CONFIG.set(config, || {
        tcx.hir().krate().visit_all_item_likes(&mut slice_visitor);
      });
      self.output = Some(slice_visitor.output);
    });

    rustc_driver::Compilation::Stop
  }
}

pub fn slice(config: Config, args: impl AsRef<str>) -> Result<SliceOutput> {
  let _ = env_logger::try_init();

  // mir-opt-level ensures that mir_promoted doesn't apply optimizations
  let args = format!("{} -Z mir-opt-level=0", args.as_ref())
    .split(" ")
    .map(str::to_string)
    .collect::<Vec<_>>();

  let mut callbacks = Callbacks {
    config: Some(config),
    output: None,
  };

  rustc_driver::catch_fatal_errors(|| rustc_driver::RunCompiler::new(&args, &mut callbacks).run())
    .map_err(|_| Error::msg("rustc panicked"))?
    .map_err(|_| Error::msg("driver failed"))?;

  callbacks.output.unwrap()
}