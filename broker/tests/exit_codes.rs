//! The helper must fail safely and say why through its exit code. These run it
//! without administrator rights, so it can never get as far as reading a disk.

use apfsreader_core::physical;
use apfsreader_core::remote::pipe::{current_user_sid, new_pipe_name, random_bytes, to_hex};
use std::process::Command;

fn run(args: &[&str]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_apfsreader-broker")).args(args).status().unwrap().code().unwrap()
}

#[test]
fn no_arguments_is_a_usage_error() {
    assert_eq!(run(&[]), 2);
}

#[test]
fn a_hostile_pipe_name_is_refused_before_anything_is_opened() {
    let token = to_hex(&random_bytes::<16>().unwrap());
    let sid = current_user_sid().unwrap();
    assert_eq!(run(&["--disk", "0", "--pipe", r"..\evil", "--token", &token, "--sid", &sid]), 2);
}

#[test]
fn without_administrator_rights_the_disk_cannot_be_opened() {
    if physical::is_elevated() {
        return; // would genuinely read the disk; this test is about the refusal
    }
    let Some(d) = physical::list_disks().into_iter().next() else { return };
    let token = to_hex(&random_bytes::<16>().unwrap());
    let sid = current_user_sid().unwrap();
    let pipe = new_pipe_name().unwrap();
    let code = run(&[
        "--disk", &d.number.to_string(), "--pipe", &pipe, "--token", &token, "--sid", &sid, "--timeout", "2",
    ]);
    assert_eq!(code, 3, "expected the 'could not open the disk' exit code");
}

#[test]
fn a_disk_number_that_does_not_exist_is_the_same_clean_failure() {
    let token = to_hex(&random_bytes::<16>().unwrap());
    let sid = current_user_sid().unwrap();
    let pipe = new_pipe_name().unwrap();
    let code = run(&["--disk", "63", "--pipe", &pipe, "--token", &token, "--sid", &sid, "--timeout", "2"]);
    assert!(code == 3, "got {code}");
}
