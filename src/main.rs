use dashmap::DashMap;
use log::debug;
use ropey::Rope;
use serde::{Deserialize, Serialize};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Notification;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use trees_lang::compile::{connect_blocks, find_blocks, CompileConfig, CompilingBlock, Edge};
#[derive(Debug)]
struct Backend {
    client: Client,
    block_map: DashMap<String, CompilingBlock>,
    blocks_map: DashMap<String, Vec<CompilingBlock>>,
    edges_map: DashMap<String, Vec<Edge>>,
    document_map: DashMap<String, Rope>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: None,
            offset_encoding: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..ServerCapabilities::default()
            },
        })
    }
    async fn initialized(&self, _: InitializedParams) {
        debug!("initialized!");
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        debug!("file opened");
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: &params.text_document.text,
            version: Some(params.text_document.version),
        })
        .await
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.on_change(TextDocumentItem {
            text: &params.content_changes[0].text,
            uri: params.text_document.uri,
            version: Some(params.text_document.version),
        })
        .await
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        dbg!(&params.text);
        if let Some(text) = params.text {
            let item = TextDocumentItem {
                uri: params.text_document.uri,
                text: &text,
                version: None,
            };
            self.on_change(item).await;
            _ = self.client.semantic_tokens_refresh().await;
        }
        debug!("file saved!");
    }
    async fn did_close(&self, _: DidCloseTextDocumentParams) {
        debug!("file closed!");
    }

    /*async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let uri = params.item.uri.to_string();
        let block = self.block_map.get(&uri).unwrap();
        let blocks = self.blocks_map.get(&uri).unwrap();
        let edges = self.edges_map.get(&uri).unwrap();

        let mut result = vec![];

        result.push(CallHierarchyIncomingCall {
            from: CallHierarchyItem {
                name: (),
                kind: (),
                tags: (),
                detail: (),
                uri: (),
                range: (),
                selection_range: (),
                data: (),
            },
            from_ranges: todo!(),
        });

        Ok(Some(result))
    }*/
}
#[derive(Debug, Deserialize, Serialize)]
struct InlayHintParams {
    path: String,
}

#[allow(unused)]
enum CustomNotification {}
impl Notification for CustomNotification {
    type Params = InlayHintParams;
    const METHOD: &'static str = "custom/notification";
}
struct TextDocumentItem<'a> {
    uri: Url,
    text: &'a str,
    version: Option<i32>,
}

impl Backend {
    fn extract_edges(edges: &mut Vec<Edge>, block: &CompilingBlock, blocks: &Vec<CompilingBlock>) {
        block.args.iter().for_each(|edge| {
            edges.push(edge.clone());
            Self::extract_edges(edges, &blocks[edge.block_index_of_block_plug], blocks);
        });
    }

    async fn on_change(&self, params: TextDocumentItem<'_>) {
        dbg!(&params.version);
        let rope = ropey::Rope::from_str(params.text);
        self.document_map
            .insert(params.uri.to_string(), rope.clone());

        let splited_code = trees_lang::compile::split_code(
            &params.text.split("\n").map(|s| s.to_string()).collect(),
            &CompileConfig::DEFAULT,
        );
        let mut tmp_blocks = find_blocks(&splited_code, &CompileConfig::DEFAULT);
        let connect_result =
            connect_blocks(&splited_code, &mut tmp_blocks, &CompileConfig::DEFAULT);

        match connect_result {
            Ok(result) => {
                dbg!("connect blocks success!");

                let mut extracted_edges = vec![];
                Self::extract_edges(&mut extracted_edges, &result, &tmp_blocks);

                self.client
                    .publish_diagnostics(params.uri.clone(), vec![], params.version)
                    .await;

                self.block_map.insert(params.uri.to_string(), result);
                self.blocks_map
                    .insert(params.uri.to_string(), tmp_blocks.clone());
                self.edges_map
                    .insert(params.uri.to_string(), extracted_edges);
            }
            Err(err) => {
                self.client
                    .publish_diagnostics(
                        params.uri.clone(),
                        vec![Diagnostic {
                            range: Range {
                                start: Position::new(0, 0),
                                end: Position::new(0, 0),
                            },
                            message: err,
                            ..Default::default()
                        }],
                        params.version,
                    )
                    .await;

                self.block_map.remove(&params.uri.to_string());
                self.blocks_map.insert(params.uri.to_string(), vec![]);
                self.edges_map.insert(params.uri.to_string(), vec![]);
            }
        };
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        client,
        block_map: DashMap::new(),
        blocks_map: DashMap::new(),
        edges_map: DashMap::new(),
        document_map: DashMap::new(),
    })
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;
}

fn offset_to_position(offset: usize, rope: &Rope) -> Option<Position> {
    let line = rope.try_char_to_line(offset).ok()?;
    let first_char_of_line = rope.try_line_to_char(line).ok()?;
    let column = offset - first_char_of_line;
    Some(Position::new(line as u32, column as u32))
}

fn position_to_offset(position: Position, rope: &Rope) -> Option<usize> {
    let line_char_offset = rope.try_line_to_char(position.line as usize).ok()?;
    let slice = rope.slice(0..line_char_offset + position.character as usize);
    Some(slice.len_bytes())
}
