use grapevine_common::{Fr, Params, G1, G2};
use nova_scotia::FileLocation;
use nova_scotia::circom::reader::load_r1cs;
use nova_scotia::circom::circuit::R1CS;
use std::env::current_dir;
use std::path::PathBuf;

// @TODO: lazy static implementation for public params and r1cs

pub fn use_public_params() -> Result<Params, Box<dyn std::error::Error>> {
    // get the path to grapevine (will create if it does not exist)
    let filepath = current_dir().unwrap().join("static/public_params.json");
    println!("Filepath: {}", filepath.display());
    // read in params file
    let public_params_file = std::fs::read_to_string(filepath).expect("Unable to read file");

    // parse file into params struct
    let public_params: Params =
        serde_json::from_str(&public_params_file).expect("Incorrect public params format");

    Ok(public_params)
}

pub fn use_r1cs() -> Result<R1CS<Fr>, Box<dyn std::error::Error>> {
    // get the path to grapevine (will create if it does not exist)
    let filepath = current_dir().unwrap().join("static/grapevine.r1cs");
    // read in params file
    Ok(load_r1cs::<G1, G2>(&FileLocation::PathBuf(filepath)))
}

// pub fn use_wasm() -> Result<PathBuf, Box<dyn std::error::Error>> {
//     // get the path to grapevine (will create if it does not exist)
//     Ok(get_storage_path().unwrap().join("grapevine.wasm"))
// }
