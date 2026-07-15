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
    // Per-node info listeners (NodeListener isn't a ProxyT).
    let live_node_listeners: Rc<RefCell<HashMap<u32, pw::node::NodeListener>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // Last latency reading per node — used to debounce LatencySample emission.
    let last_latency_ms: Rc<RefCell<HashMap<u32, f32>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // Last xrun-count per node — diffed against info events to emit one
    // BackendEvent::Xrun per new xrun. Misses nodes that don't advertise
    // `xrun-count` in props (most ALSA/PW-native); for those we'd need
    // the Profiler POD path. Catches JACK clients.
    let last_xrun_count: Rc<RefCell<HashMap<u32, u64>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // Port id → owning node id. Populated as Port globals arrive so
    // PwCmd::CreateLink can pass valid node ids to PipeWire's link
    // factory (which silently no-ops when node ids are 0).
    let port_to_node: Rc<RefCell<HashMap<u32, u32>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // Track the kind of each global so global_remove can emit the
    // right BackendEvent variant. PipeWire's global_remove only gives
    // an id, not a type.
    #[derive(Copy, Clone)]
    enum GlobalKind { Node, Port, Link }
    let global_kinds: Rc<RefCell<HashMap<u32, GlobalKind>>> =
        Rc::new(RefCell::new(HashMap::new()));

    // Handle commands from the outside world.
    let loop_ = mainloop.loop_();
    let mainloop_for_cmd = mainloop.clone();
    let core_for_cmd = core.clone();
    let _live_proxies_for_cmd = live_proxies.clone();
    let live_links_for_cmd = live_links.clone();
    let port_to_node_for_cmd = port_to_node.clone();
    // Track link factory name.
    let factory_name: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let factory_name_cmd = factory_name.clone();

    let _cmd_attached = cmd_rx.attach(loop_, move |cmd| match cmd {
        PwCmd::Quit => mainloop_for_cmd.quit(),
        PwCmd::CreateLink { output_port, input_port, output_node, input_node } => {
            tracing::info!(
                output_port, input_port, output_node, input_node,
                p2n_size = port_to_node_for_cmd.borrow().len(),
                "PipeWire: PwCmd::CreateLink received in loop"
            );
            // The daemon passes node hints of 0 because the Link wire
            // type carries only port ids. Look the real node ids up
            // from the registry-fed port→node map. Without valid node
            // ids PW's link-factory silently fails to register the
            // new link in the global registry, so no LinkAppeared
            // event ever fires.
            let resolved = {
                let p2n = port_to_node_for_cmd.borrow();
                resolve_link_endpoints(
                    &p2n, output_port, input_port, output_node, input_node,
                )
            };
            let (resolved_out, resolved_in) = match resolved {
                Some(pair) => pair,
                None => {
                    tracing::warn!(
                        output_port, input_port,
                        p2n_has_out = port_to_node_for_cmd.borrow().contains_key(&output_port),
                        p2n_has_in  = port_to_node_for_cmd.borrow().contains_key(&input_port),
                        "PipeWire: create_link skipped — unknown owning node for port(s)"
                    );
                    return;
                }
            };
            tracing::info!(
                output_port, input_port, resolved_out, resolved_in,
                factory = ?factory_name_cmd.borrow().as_deref(),
                "PipeWire: calling create_object"
            );
            if let Some(factory) = factory_name_cmd.borrow().as_deref() {
                match core_for_cmd.create_object::<pw::link::Link>(
                    factory,
                    &pw::properties::properties! {
                        "link.output.port" => output_port.to_string(),
                        "link.input.port"  => input_port.to_string(),
                        "link.output.node" => resolved_out.to_string(),
                        "link.input.node"  => resolved_in.to_string(),
                        "object.linger"    => "1"
                    },
                ) {
                    Ok(link) => {
                        let pid = link.upcast_ref().id();
                        tracing::info!(
                            proxy_id = pid,
                            "PipeWire: create_object Ok — link proxy stored"
                        );
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
    let listeners_for_reg = live_node_listeners.clone();
    let last_latency_for_reg = last_latency_ms.clone();
    let last_xrun_for_reg = last_xrun_count.clone();
    let factory_for_reg = factory_name.clone();
    let port_to_node_for_reg = port_to_node.clone();
    let port_to_node_for_remove = port_to_node.clone();
    let global_kinds_for_reg = global_kinds.clone();
    let global_kinds_for_remove = global_kinds.clone();
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
                    global_kinds_for_reg
                        .borrow_mut()
                        .insert(global.id, GlobalKind::Node);
                    broadcast(&sinks, BackendEvent::NodeAppeared(node));

                    let node_proxy: pw::node::Node = reg.bind(global).unwrap();

                    // Subscribe to info events so we can extract latency
                    // readings from `node.latency` props as they arrive.
                    // Real xrun reporting requires binding to the PW
                    // Profiler global and parsing its POD payload — the
                    // pipewire 0.10 crate doesn't wrap Profiler, so that
                    // path stays a manual SPA POD task for v0.5+.
                    let sinks_info = sinks.clone();
                    let last_lat = last_latency_for_reg.clone();
                    let last_xrun = last_xrun_for_reg.clone();
                    let node_id = global.id;
                    let listener = node_proxy
                        .add_listener_local()
                        .info(move |info| {
                            let Some(props) = info.props() else { return };

                            if let Some(lat_str) = props.get("node.latency") {
                                if let Some(ms) = parse_latency_ms(lat_str) {
                                    let mut last = last_lat.borrow_mut();
                                    if last.get(&node_id).copied() != Some(ms) {
                                        last.insert(node_id, ms);
                                        broadcast(
                                            &sinks_info,
                                            BackendEvent::LatencySample {
                                                node_id: NodeId(node_id as u64),
                                                latency_ms: ms,
                                            },
                                        );
                                    }
                                }
                            }

                            // Some nodes (notably JACK clients) advertise
                            // a cumulative xrun counter on info props.
                            // On the first observation we record the
                            // baseline silently; subsequent increases
                            // emit one Xrun per delta.
                            if let Some(xrun_str) = props.get("xrun-count") {
                                if let Ok(now) = xrun_str.trim().parse::<u64>() {
                                    let mut last = last_xrun.borrow_mut();
                                    match last.get(&node_id).copied() {
                                        None => {
                                            last.insert(node_id, now);
                                        }
                                        Some(prev) if now > prev => {
                                            last.insert(node_id, now);
                                            for _ in 0..(now - prev) {
                                                broadcast(
                                                    &sinks_info,
                                                    BackendEvent::Xrun {
                                                        node_id: NodeId(node_id as u64),
                                                        gap_ms: 0.0,
                                                    },
                                                );
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        })
                        .register();
                    listeners_for_reg.borrow_mut().insert(global.id, listener);
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
                    port_to_node_for_reg
                        .borrow_mut()
                        .insert(global.id, node_id as u32);
                    global_kinds_for_reg
                        .borrow_mut()
                        .insert(global.id, GlobalKind::Port);
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
                    global_kinds_for_reg
                        .borrow_mut()
                        .insert(global.id, GlobalKind::Link);
                    broadcast(&sinks, BackendEvent::LinkAppeared(link));
                }
                _ => {}
            }
        })
        .global_remove(move |id| {
            // Dispatch on the kind we recorded when the global appeared.
            // Before the fix this branch always fired NodeRemoved, even
            // for Port and Link removals — confusing the daemon's graph.
            let kind = global_kinds_for_remove.borrow_mut().remove(&id);
            match kind {
                Some(GlobalKind::Node) => {
                    broadcast(
                        &sinks_remove,
                        BackendEvent::NodeRemoved(NodeId(id as u64)),
                    );
                }
                Some(GlobalKind::Port) => {
                    port_to_node_for_remove.borrow_mut().remove(&id);
                    broadcast(
                        &sinks_remove,
                        BackendEvent::PortRemoved(PortId(id as u64)),
                    );
                }
                Some(GlobalKind::Link) => {
                    broadcast(
                        &sinks_remove,
                        BackendEvent::LinkRemoved(
                            soundworm_core::link::LinkId(id as u64),
                        ),
                    );
                }
                None => {}
            }
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

/// Resolve PipeWire link endpoints: caller may have node hints (non-zero)
/// or zeros to fall back on the port→node map. Returns the (output_node,
/// input_node) pair to pass to the link-factory, or `None` if either
/// endpoint can't be resolved. Pure function so it can be unit tested
/// without spinning up a real PipeWire main loop.
fn resolve_link_endpoints(
    port_to_node: &HashMap<u32, u32>,
    output_port: u32,
    input_port: u32,
    output_node_hint: u32,
    input_node_hint: u32,
) -> Option<(u32, u32)> {
    let resolved_out = if output_node_hint != 0 {
        output_node_hint
    } else {
        port_to_node.get(&output_port).copied().unwrap_or(0)
    };
    let resolved_in = if input_node_hint != 0 {
        input_node_hint
    } else {
        port_to_node.get(&input_port).copied().unwrap_or(0)
    };
    if resolved_out == 0 || resolved_in == 0 {
        return None;
    }
    Some((resolved_out, resolved_in))
}

/// Parse PipeWire's `node.latency` prop, formatted `"samples/rate"`,
/// into milliseconds. Returns `None` if the format is unexpected or
/// rate is zero.
fn parse_latency_ms(spec: &str) -> Option<f32> {
    let (samples, rate) = spec.split_once('/')?;
    let samples: f32 = samples.trim().parse().ok()?;
    let rate: f32 = rate.trim().parse().ok()?;
    if rate <= 0.0 { return None; }
    Some(samples / rate * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_node_latency() {
        assert!((parse_latency_ms("1024/48000").unwrap() - 21.333_334).abs() < 0.01);
        assert!((parse_latency_ms("512/96000").unwrap() - 5.333_333).abs() < 0.01);
    }

    #[test]
    fn rejects_garbage_latency() {
        assert!(parse_latency_ms("").is_none());
        assert!(parse_latency_ms("1024").is_none());
        assert!(parse_latency_ms("1024/0").is_none());
        assert!(parse_latency_ms("abc/48000").is_none());
    }

    #[test]
    fn resolves_endpoints_from_port_to_node_map() {
        let mut p2n = HashMap::new();
        p2n.insert(100, 10);  // port 100 belongs to node 10
        p2n.insert(200, 20);  // port 200 belongs to node 20

        let (out_node, in_node) =
            resolve_link_endpoints(&p2n, 100, 200, 0, 0).expect("should resolve");
        assert_eq!(out_node, 10);
        assert_eq!(in_node, 20);
    }

    #[test]
    fn resolve_endpoints_honors_explicit_hints() {
        let mut p2n = HashMap::new();
        p2n.insert(100, 10);
        // Caller-supplied non-zero hints win over the map lookup.
        let (out_node, in_node) =
            resolve_link_endpoints(&p2n, 100, 200, 999, 888).expect("should resolve");
        assert_eq!(out_node, 999);
        assert_eq!(in_node, 888);
    }

    #[test]
    fn resolve_endpoints_returns_none_for_unknown_port() {
        let p2n: HashMap<u32, u32> = HashMap::new();
        // Both ports unknown and no hints given.
        assert!(resolve_link_endpoints(&p2n, 100, 200, 0, 0).is_none());
    }

    #[test]
    fn resolve_endpoints_returns_none_when_only_one_resolves() {
        let mut p2n = HashMap::new();
        p2n.insert(100, 10);
        // Input port is unknown — PW's factory would silently fail
        // to register the link, so we'd rather catch it here.
        assert!(resolve_link_endpoints(&p2n, 100, 999, 0, 0).is_none());
    }

    #[test]
    fn resolve_endpoints_treats_zero_node_id_as_invalid() {
        let mut p2n = HashMap::new();
        // Port 100 maps to node id 0 — unusable as a PW node ref.
        p2n.insert(100, 0);
        p2n.insert(200, 20);
        assert!(resolve_link_endpoints(&p2n, 100, 200, 0, 0).is_none());
    }
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
        tracing::info!(output_port, input_port, "PipeWire: queue CreateLink");
        // node IDs aren't in Link; pass 0 — the PW thread resolves
        // them from its port→node map. See resolve_link_endpoints.
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
