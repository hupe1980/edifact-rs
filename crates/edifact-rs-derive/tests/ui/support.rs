pub mod edifact_rs {
    #[derive(Debug)]
    pub enum EdifactError {
        MissingRequiredElement { tag: String, element_index: usize },
        MissingSegment { tag: String, expected_position: String },
    }

    #[derive(Debug, Clone, Copy)]
    pub enum EdifactEvent<'a> {
        StartSegment { tag: &'a str },
        Element { value: &'a str },
        ComponentElement { value: &'a str },
        EndSegment,
    }

    pub trait EventEmitter {
        fn emit(&mut self, _event: EdifactEvent<'_>) -> Result<(), EdifactError>;
    }

    pub trait EdifactSerialize {
        fn edifact_serialize<E: EventEmitter>(
            &self,
            emitter: &mut E,
        ) -> Result<(), EdifactError>;
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
                return emitter.emit(EdifactEvent::Element { value: "" });
            }
            emitter.emit(EdifactEvent::Element { value: &self[0] })?;
            for component in self.iter().skip(1) {
                emitter.emit(EdifactEvent::ComponentElement { value: component })?;
            }
            Ok(())
        }
    }

    pub struct Element<'a> {
        _marker: std::marker::PhantomData<&'a ()>,
    }

    impl<'a> Element<'a> {
        pub fn get_component(&self, _n: usize) -> Option<&'a str> {
            None
        }
    }

    pub struct Segment<'a> {
        pub tag: &'a str,
    }

    impl<'a> Segment<'a> {
        pub fn element_str(&self, _n: usize) -> Option<&'a str> {
            None
        }

        pub fn get_element(&self, _n: usize) -> Option<&Element<'a>> {
            None
        }
    }

    pub struct OwnedElement {
        pub components: Vec<String>,
    }

    pub struct OwnedSegment {
        pub tag: String,
        pub elements: Vec<OwnedElement>,
    }

    impl OwnedSegment {
        pub fn element_str(&self, n: usize) -> Option<&str> {
            self.elements.get(n)?.components.first().map(|s| s.as_str())
        }
        pub fn component_str(&self, elem: usize, comp: usize) -> Option<&str> {
            self.elements.get(elem)?.components.get(comp).map(|s| s.as_str())
        }
    }

    pub trait EdifactSegmentTag {
        const SEGMENT_TAG: &'static str;

        fn matches_segment(seg: &Segment<'_>) -> bool {
            seg.tag == Self::SEGMENT_TAG
        }

        fn matches_owned_segment(seg: &OwnedSegment) -> bool {
            seg.tag == Self::SEGMENT_TAG
        }
    }

    pub trait EdifactDeserialize: Sized {
        fn edifact_deserialize(_segments: &[Segment<'_>]) -> Result<Self, EdifactError>;
        fn edifact_deserialize_owned(_segments: &[OwnedSegment]) -> Result<Self, EdifactError> {
            unimplemented!()
        }
    }

    pub struct CompositeElement<'a> {
        components: Vec<std::borrow::Cow<'a, str>>,
    }

    impl<'a> CompositeElement<'a> {
        pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
            self.components.iter().map(|c| c.as_ref())
        }

        pub fn from_slice(components: &'a [std::borrow::Cow<'a, str>]) -> Self {
            Self { components: components.iter().cloned().collect() }
        }
    }

    pub trait EdifactCompositeDeserialize: Sized {
        fn edifact_deserialize_composite(composite: CompositeElement<'_>) -> Result<Self, EdifactError>;
    }

    impl EdifactCompositeDeserialize for Vec<String> {
        fn edifact_deserialize_composite(composite: CompositeElement<'_>) -> Result<Self, EdifactError> {
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
        segments.iter().find(|segment| {
            segment.tag == tag && segment.element_str(0).unwrap_or("") == qualifier
        })
    }

    pub fn find_segment_owned<'s>(
        segments: &'s [OwnedSegment],
        tag: &str,
    ) -> Option<&'s OwnedSegment> {
        segments.iter().find(|s| s.tag == tag)
    }

    pub fn find_qualified_segment_owned<'s>(
        segments: &'s [OwnedSegment],
        tag: &str,
        qualifier: &str,
    ) -> Option<&'s OwnedSegment> {
        segments.iter().find(|s| {
            s.tag == tag && s.element_str(0).unwrap_or("") == qualifier
        })
    }

    pub fn find_segments_typed<'s, 'd: 's, T>(
        segments: &'s [Segment<'d>],
    ) -> impl Iterator<Item = &'s Segment<'d>>
    where
        T: EdifactSegmentTag,
    {
        segments.iter().filter(|s| T::matches_segment(s))
    }
}