// soundworm routing script
// node_name is injected automatically

if node_name == "spotify" || node_name == "vlc" {
    log_route(node_name, "speakers");
    allow()
} else if node_name == "zoom" || node_name == "teams" {
    log_route(node_name, "usb_headset");
    allow()
} else {
    deny()
}
