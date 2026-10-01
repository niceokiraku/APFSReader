fn main() {
    // The mount library links WinFsp by delay-load; the executable must carry the same link arguments.
    winfsp::build::winfsp_link_delayload();
    apfsreader_build_resources::embed("APFSReader");
}
