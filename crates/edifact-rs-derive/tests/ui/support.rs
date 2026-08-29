//! A minimal stand-in for `edifact_rs`, so the UI suite can compile the derive
//! output without a dependency cycle.
//!
//! Every item here mirrors the real crate's shape closely enough that generated
//! code type-checks against it. Keep it in step with what the derive actually
//! emits — `grep -o "::edifact_rs::[A-Za-z_]*" src/lib.rs` lists the surface.

pub mod edifact_rs {
    use std::borrow::Cow;

    #[derive(Debug)]
    pub enum EdifactError {
        MissingRequiredElement {
            tag: String,
            element_index: usize,
        },
        MissingRequiredComponent {
            tag: String,
            element_index: usize,
            component_index: usize,
        },
        MissingSegment {
            tag: String,
            expected_position: String,
        },
        InvalidFieldValue {
            tag: String,
            element_index: usize,
            value: String,
        },
        InvalidText {
            offset: usize,
        },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum EdifactEvent<'a> {
        StartSegment { tag: Cow<'a, str> },
        Element { value: Cow<'a, str> },
        ComponentElement { value: Cow<'a, str> },
        RepeatElement { value: Cow<'a, str> },
        EndSegment,
    }

    impl<'a> EdifactEvent<'a> {
        pub fn start(tag: impl Into<Cow<'a, str>>) -> Self {
            Self::StartSegment { tag: tag.into() }
        }
        pub fn element(value: impl Into<Cow<'a, str>>) -> Self {
            Self::Element {
                value: value.into(),
            }
        }
        pub fn component(value: impl Into<Cow<'a, str>>) -> Self {
            Self::ComponentElement {
                value: value.into(),
            }
        }
        pub fn repeat(value: impl Into<Cow<'a, str>>) -> Self {
            Self::RepeatElement {
                value: value.into(),
            }
        }
    }

    impl EdifactEvent<'_> {
        pub fn into_owned(self) -> EdifactEvent<'static> {
            match self {
                Self::StartSegment { tag } => EdifactEvent::StartSegment {
                    tag: Cow::Owned(tag.into_owned()),
                },
                Self::Element { value } => EdifactEvent::Element {
                    value: Cow::Owned(value.into_owned()),
                },
                Self::ComponentElement { value } => EdifactEvent::ComponentElement {
                    value: Cow::Owned(value.into_owned()),
                },
                Self::RepeatElement { value } => EdifactEvent::RepeatElement {
                    value: Cow::Owned(value.into_owned()),
                },
                Self::EndSegment => EdifactEvent::EndSegment,
            }
        }
    }

    pub trait EventEmitter {
        fn emit(&mut self, _event: EdifactEvent<'_>) -> Result<(), EdifactError>;
        fn decimal_mark(&self) -> u8 {
            b'.'
        }
    }

    #[derive(Debug, Default)]
    pub struct VecEmitter {
        pub events: Vec<EdifactEvent<'static>>,
    }

    impl EventEmitter for VecEmitter {
        fn emit(&mut self, event: EdifactEvent<'_>) -> Result<(), EdifactError> {
            self.events.push(event.into_owned());
            Ok(())
        }
    }

    pub fn emit_sparse_segment<E: EventEmitter>(
        emitter: &mut E,
        tag: &str,
        parts: &mut [(usize, usize, Cow<'_, str>)],
    ) -> Result<(), EdifactError> {
        emitter.emit(EdifactEvent::start(tag))?;
        for (_, component, value) in parts.iter() {
            let event = if *component == 0 {
                EdifactEvent::element(value.as_ref())
            } else {
                EdifactEvent::component(value.as_ref())
            };
            emitter.emit(event)?;
        }
        emitter.emit(EdifactEvent::EndSegment)
    }

    pub trait EdifactSerialize {
        fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError>;
    }

    pub trait EdifactCompositeSerialize {
        fn edifact_serialize_composite<E: EventEmitter>(
            &self,
            emitter: &mut E,
        ) -> Result<(), EdifactError>;
    }

    impl EdifactCompositeSerialize for Vec<String> {
        fn edifact_serialize_composite<E: EventEmitter>(
            &self,
            emitter: &mut E,
        ) -> Result<(), EdifactError> {
            if self.is_empty() {
                return emitter.emit(EdifactEvent::element(""));
            }
            emitter.emit(EdifactEvent::element(&self[0]))?;
            for component in self.iter().skip(1) {
                emitter.emit(EdifactEvent::component(component))?;
            }
            Ok(())
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    pub struct Span {
        pub start: usize,
        pub end: usize,
    }

    pub struct Element<'a> {
        _marker: std::marker::PhantomData<&'a ()>,
    }

    impl<'a> Element<'a> {
        pub fn get_component(&self, _n: usize) -> Option<&'a str> {
            None
        }
    }

    /// One segment type, borrowed or owned — as in the real crate.
    pub struct Segment<'a> {
        pub tag: Cow<'a, str>,
        pub span: Span,
    }

    pub type OwnedSegment = Segment<'static>;

    impl<'a> Segment<'a> {
        pub fn tag(&self) -> &str {
            self.tag.as_ref()
        }

        pub fn element_str(&self, _n: usize) -> Option<&str> {
            None
        }

        pub fn get_element(&self, _n: usize) -> Option<&Element<'a>> {
            None
        }

        pub fn repeated_component(
            &self,
            _element: usize,
            _component: usize,
        ) -> impl Iterator<Item = &str> {
            std::iter::empty()
        }
    }

    pub trait EdifactSegmentTag {
        const SEGMENT_TAG: &'static str;
        const QUALIFIER_PATTERN: Option<&'static str> = None;

        fn matches_segment(seg: &Segment<'_>) -> bool {
            seg.tag == Self::SEGMENT_TAG
        }
    }

    pub trait EdifactDeserialize: Sized {
        fn edifact_deserialize(_segments: &[Segment<'_>]) -> Result<Self, EdifactError>;
    }

    pub struct CompositeElement<'a> {
        components: Vec<Cow<'a, str>>,
    }

    impl<'a> CompositeElement<'a> {
        pub fn get(&self, i: usize) -> Option<&str> {
            self.components.get(i).map(|c| c.as_ref())
        }

        pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
            self.components.iter().map(|c| c.as_ref())
        }

        pub fn from_slice(components: &'a [Cow<'a, str>]) -> Self {
            Self {
                components: components.to_vec(),
            }
        }
    }

    pub trait EdifactCompositeDeserialize: Sized {
        fn edifact_deserialize_composite(
            composite: CompositeElement<'_>,
        ) -> Result<Self, EdifactError>;
    }

    impl EdifactCompositeDeserialize for Vec<String> {
        fn edifact_deserialize_composite(
            composite: CompositeElement<'_>,
        ) -> Result<Self, EdifactError> {
            Ok(composite.iter().map(str::to_owned).collect())
        }
    }

    pub fn composite_element<'s, 'd>(
        _seg: &'s Segment<'d>,
        _idx: usize,
    ) -> Option<CompositeElement<'s>> {
        None
    }

    pub fn find_segment<'s, 'd>(segments: &'s [Segment<'d>], tag: &str) -> Option<&'s Segment<'d>> {
        segments.iter().find(|segment| segment.tag == tag)
    }

    pub fn find_qualified_segment<'s, 'd>(
        segments: &'s [Segment<'d>],
        tag: &str,
        qualifier: &str,
    ) -> Option<&'s Segment<'d>> {
        segments
            .iter()
            .find(|segment| segment.tag == tag && segment.element_str(0).unwrap_or("") == qualifier)
    }

    pub fn find_segments_typed<'s, 'd: 's, T>(
        segments: &'s [Segment<'d>],
    ) -> impl Iterator<Item = &'s Segment<'d>>
    where
        T: EdifactSegmentTag,
    {
        segments.iter().filter(|s| T::matches_segment(s))
    }

    pub mod helpers {
        // Only a couple of UI cases reach for these, so in every other case
        // rustc emits an unused-import warning — which trybuild captures into
        // the blessed `.stderr`.  The *note* rustc attaches to that warning has
        // been reworded since the MSRV, so it was the sole reason 24 of 26
        // expectations mismatched on any newer toolchain, burying real
        // regressions in noise.  Silencing it keeps the expectations to the
        // derive's own diagnostics, which are stable across toolchains.
        #[allow(unused_imports)]
        pub use super::{
            composite_element, find_qualified_segment, find_segment, find_segments_typed,
        };
    }
}
