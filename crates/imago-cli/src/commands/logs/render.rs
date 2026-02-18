use imago_protocol::LogChunk;

pub(crate) trait LogRenderer {
    fn render_chunk(
        &self,
        chunk: &LogChunk,
        all_processes: bool,
        output_format: super::LogsOutputFormat,
        prefix_state: &mut super::PrefixRenderState,
        json_state: &mut super::JsonLinesRenderState,
    ) -> anyhow::Result<()>;

    fn flush_tail(
        &self,
        output_format: super::LogsOutputFormat,
        json_state: &mut super::JsonLinesRenderState,
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DefaultLogRenderer;

impl LogRenderer for DefaultLogRenderer {
    fn render_chunk(
        &self,
        chunk: &LogChunk,
        all_processes: bool,
        output_format: super::LogsOutputFormat,
        prefix_state: &mut super::PrefixRenderState,
        json_state: &mut super::JsonLinesRenderState,
    ) -> anyhow::Result<()> {
        super::render_chunk(
            chunk,
            all_processes,
            output_format,
            prefix_state,
            json_state,
        )
    }

    fn flush_tail(
        &self,
        output_format: super::LogsOutputFormat,
        json_state: &mut super::JsonLinesRenderState,
    ) -> anyhow::Result<()> {
        super::flush_json_tail_if_needed(output_format, json_state)
    }
}
