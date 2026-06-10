use async_trait::async_trait;
use pipewire as pw;
use pw::{proxy::ProxyT, types::ObjectType};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{mpsc, Arc, Mutex},
    thread,
};
use soundworm_core::{
    backend::AudioBackend,
    error::{Result, SoundwormError},
    event::BackendEvent,
    link::Link,
    node::{Node, NodeId, NodeKind},
    port::{Direction, Port, PortId},
};

// Commands sent from the main thread into the PipeWire loop thread.
enum PwCmd {
    CreateLink { output_port: u32, input_port: u32, output_node: u32, input_node: u32 },
    DestroyLink { proxy_id: u32 },
    Quit,
}

pub struct PipeWireBackend {
    // Cloneable sender for commands into the PW thread.
    cmd_tx:    pw::channel::Sender<PwCmd>,
    // Shared snapshot of the current graph state.
    nodes:     Arc<Mutex<HashMap<u32, Node>>>,
    // Template receiver for new subscribers — only one consumer at a time; we use a broadcast
    // pattern via a Vec of senders.
    event_sinks: Arc<Mutex<Vec<mpsc::SyncSender<BackendEvent>>>>,
}

impl PipeWireBackend {
    pub fn new() -> anyhow::Result<Self> {
        pw::init();

        let (cmd_tx, cmd_rx) = pw::channel::channel::<PwCmd>();
        let nodes_shared: Arc<Mutex<HashMap<u32, Node>>> = Arc::new(Mutex::new(HashMap::new()));
        let event_sinks: Arc<Mutex<Vec<mpsc::SyncSender<BackendEvent>>>> =
            Arc::new(Mutex::new(Vec::new()));

        let nodes_clone = nodes_shared.clone();
        let sinks_clone = event_sinks.clone();

        thread::Builder::new()
            .name("soundworm-pipewire".into())
            .spawn(move || pw_thread(cmd_rx, nodes_clone, sinks_clone))?;

        Ok(Self {
            cmd_tx,
            nodes: nodes_shared,
            event_sinks,
        })
    }
}

impl Default for PipeWireBackend {
    fn default() -> Self {
        Self::new().expect("PipeWire backend init failed")
    }
}

// Broadcast an event to all registered subscribers, pruning dead ones.
fn broadcast(sinks: &Arc<Mutex<Vec<mpsc::SyncSender<BackendEvent>>>>, event: BackendEvent) {
    let mut guard = sinks.lock().unwrap();
    guard.retain(|tx| tx.try_send(event.clone()).is_ok());
}

fn pw_thread(
    cmd_rx: pw::channel::Receiver<PwCmd>,
    nodes: Arc<Mutex<HashMap<u32, Node>>>,
    sinks: Arc<Mutex<Vec<mpsc::SyncSender<BackendEvent>>>>,
) {
    let mainloop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(ml) => ml,
        Err(e) => { tracing::error!("PipeWire: mainloop creation failed: {}", e); return; }
    };
    let context = match pw::context::ContextRc::new(&mainloop, None) {
        Ok(c) => c,
        Err(e) => { tracing::error!("PipeWire: context creation failed: {}", e); return; }
    };
    let core = match context.connect_rc(None) {
        Ok(c) => c,
        Err(e) => { tracing::error!("PipeWire: connect failed: {}", e); return; }
    };
    let registry = match core.get_registry_rc() {
        Ok(r) => r,
        Err(e) => { tracing::error!("PipeWire: get_registry failed: {}", e); return; }
    };

    // Keep live proxies around so PW doesn't remove them.
    let live_proxies: Rc<RefCell<HashMap<u32, Box<dyn pw::proxy::ProxyT>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // Separate map for link proxies we may need to destroy.
    let live_links: Rc<RefCell<HashMap<u32, pw::link::Link>>> =
        Rc::new(RefCell::new(HashMap::new()));

    // Handle commands from the outside world.
    let loop_ = mainloop.loop_();
    let mainloop_for_cmd = mainloop.clone();
    let core_for_cmd = core.clone();
    let _live_proxies_for_cmd = live_proxies.clone();
    let live_links_for_cmd = live_links.clone();
    // Track link factory name.
    let factory_name: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let factory_name_cmd = factory_name.clone();

    let _cmd_attached = cmd_rx.attach(loop_, move |cmd| match cmd {
        PwCmd::Quit => mainloop_for_cmd.quit(),
        PwCmd::CreateLink { output_port, input_port, output_node, input_node } => {
            if let Some(factory) = factory_name_cmd.borrow().as_deref() {
                match core_for_cmd.create_object::<pw::link::Link>(
                    factory,
                    &pw::properties::properties! {
                        "link.output.port" => output_port.to_string(),
                        "link.input.port"  => input_port.to_string(),
                        "link.output.node" => output_node.to_string(),
                        "link.input.node"  => input_node.to_string(),
                        "object.linger"    => "1"
                    },
                ) {
                    Ok(link) => {
                        let pid = link.upcast_ref().id();
                        live_links_for_cmd.borrow_mut().insert(pid, link);
                    }
                    Err(e) => tracing::error!("PipeWire: create_link failed: {}", e),
                }
            } else {
                tracing::warn!("PipeWire: no link factory known yet");
            }
        }
        PwCmd::DestroyLink { proxy_id } => {
            if let Some(link) = live_links_for_cmd.borrow_mut().remove(&proxy_id) {
                if let Err(e) = core_for_cmd.destroy_object(link) {
                    tracing::error!("PipeWire: destroy_link failed: {}", e);
                }
            }
        }
    });

    let registry_weak = registry.downgrade();
    let live_for_reg = live_proxies.clone();
    let factory_for_reg = factory_name.clone();
    let sinks_remove = sinks.clone();

    let _reg_listener = registry
        .add_listener_local()
        .global(move |global| {
            let reg = match registry_weak.upgrade() { Some(r) => r, None => return };

            // Track link factory name.
            if global.type_ == ObjectType::Factory {
                if let Some(props) = global.props.as_ref() {
                    if props.get("factory.type.name") == Some(ObjectType::Link.to_str()) {
                        if let Some(name) = props.get("factory.name") {
                            *factory_for_reg.borrow_mut() = Some(name.to_owned());
                        }
                    }
                }
            }

            match global.type_ {
                ObjectType::Node => {
                    let props = global.props.as_ref();
                    let name = props
                        .and_then(|p| p.get("node.name"))
                        .unwrap_or("unknown")
                        .to_owned();
                    let app_name = props.and_then(|p| p.get("application.name")).map(str::to_owned);
                    let media_class = props
                        .and_then(|p| p.get("media.class"))
                        .unwrap_or("")
                        .to_owned();
                    let kind = media_class_to_kind(&media_class);
                    let node = Node {
                        id: NodeId(global.id as u64),
                        name,
                        kind,
                        app_name,
                        media_class,
                        sample_rate: 48000,
                        channels: 2,
                        latency_ms: 0.0,
                        properties: HashMap::new(),
                    };
                    nodes.lock().unwrap().insert(global.id, node.clone());
                    broadcast(&sinks, BackendEvent::NodeAppeared(node));

                    let node_proxy: pw::node::Node = reg.bind(global).unwrap();
                    live_for_reg.borrow_mut().insert(global.id, Box::new(node_proxy));
                }
                ObjectType::Port => {
                    let props = global.props.as_ref();
                    let name = props
                        .and_then(|p| p.get("port.name"))
                        .unwrap_or("unknown")
                        .to_owned();
                    let dir_str = props.and_then(|p| p.get("port.direction")).unwrap_or("");
                    let direction = if dir_str == "out" { Direction::Output } else { Direction::Input };
                    let node_id = props
                        .and_then(|p| p.get("node.id"))
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    let port = Port {
                        id: PortId(global.id as u64),
                        node_id: NodeId(node_id),
                        name,
                        direction,
                        channels: 1,
                    };
                    broadcast(&sinks, BackendEvent::PortAppeared(port));

                    let port_proxy: pw::port::Port = reg.bind(global).unwrap();
                    live_for_reg.borrow_mut().insert(global.id, Box::new(port_proxy));
                }
                ObjectType::Link => {
                    let props = global.props.as_ref();
                    let src = props.and_then(|p| p.get("link.output.port"))
                        .and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                    let dst = props.and_then(|p| p.get("link.input.port"))
                        .and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                    let link = Link {
                        id: soundworm_core::link::LinkId(global.id as u64),
                        source_port: PortId(src),
                        sink_port: PortId(dst),
                        latency_compensation_ms: 0.0,
                    };
                    broadcast(&sinks, BackendEvent::LinkAppeared(link));
                }
                _ => {}
            }
        })
        .global_remove(move |id| {
            broadcast(&sinks_remove, BackendEvent::NodeRemoved(NodeId(id as u64)));
        })
        .register();

    // Do an initial sync so callers know when enumeration is done.
    let done = Rc::new(Cell::new(false));
    let done_clone = done.clone();
    let ml_clone = mainloop.clone();
    let pending = core.sync(0).expect("sync failed");
    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                done_clone.set(true);
                ml_clone.quit();
            }
        })
        .register();
    while !done.get() { mainloop.run(); }

    // Run the main loop indefinitely, handling events.
    mainloop.run();
}

fn media_class_to_kind(mc: &str) -> NodeKind {
    if mc.contains("Sink") || mc.contains("Output") { NodeKind::Sink }
    else if mc.contains("Source") || mc.contains("Input") { NodeKind::Source }
    else { NodeKind::Filter }
}

#[async_trait]
impl AudioBackend for PipeWireBackend {
    fn name(&self) -> &str { "pipewire" }

    fn subscribe(&self) -> mpsc::Receiver<BackendEvent> {
        let (tx, rx) = mpsc::sync_channel(256);
        self.event_sinks.lock().unwrap().push(tx);
        rx
    }

    async fn enumerate_nodes(&self) -> Result<Vec<Node>> {
        Ok(self.nodes.lock().unwrap().values().cloned().collect())
    }

    async fn create_link(&self, link: &Link) -> Result<()> {
        let output_port = link.source_port.0 as u32;
        let input_port  = link.sink_port.0 as u32;
        // node IDs aren't in Link; pass 0 — PW can resolve via port IDs
        self.cmd_tx.send(PwCmd::CreateLink {
            output_port, input_port, output_node: 0, input_node: 0,
        }).map_err(|_| SoundwormError::Backend("PW thread closed".into()))
    }

    async fn destroy_link(&self, link: &Link) -> Result<()> {
        self.cmd_tx.send(PwCmd::DestroyLink { proxy_id: link.id.0 as u32 })
            .map_err(|_| SoundwormError::Backend("PW thread closed".into()))
    }

    async fn set_volume(&self, _node_id: u64, _volume: f32) -> Result<()> {
        Ok(()) // v0.4 scope
    }
}

impl Drop for PipeWireBackend {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(PwCmd::Quit);
    }
}
