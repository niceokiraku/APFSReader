fn main() {
    // WinFsp is loaded at run time from its installation, not linked in.
    winfsp::build::winfsp_link_delayload();
    apfsreader_build_resources::embed("APFSReader volume mounter (WinFsp)");
}
