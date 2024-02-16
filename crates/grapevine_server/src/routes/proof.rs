use crate::guards::NonceGuard;
use crate::mongo::GrapevineDB;
use crate::utils::use_public_params;
use grapevine_circuits::{nova::verify_nova_proof, utils::decompress_proof};
use grapevine_common::{
    http::requests::{DegreeProofRequest, NewPhraseRequest},
    models::proof::{DegreeProof, ProvingData},
};
use mongodb::bson::oid::ObjectId;
use rocket::{
    data::ToByteUnit, http::Status, serde::json::Json, tokio::io::AsyncReadExt, Data, State,
};
use std::str::FromStr;

/// POST REQUESTS ///

/**
 * Create a new phrase and (a degree 1 proof) and add it to the database
 *
 * @param data - binary serialized NewPhraseRequest containing:
 *             * username: the username of the user creating the phrase
 *             * proof: the gzip-compressed fold proof
 *        
 * @return status:
 *             * 201 if success
 *             * 400 if proof verification failed, deserialization fails, or proof decompression
 *               fails
 *             * 401 if signature mismatch or nonce mismatch
 *             * 404 if user not found
 *             * 500 if db fails or other unknown issue
 */
#[post("/phrase/create", data = "<data>")]
pub async fn create_phrase(
    _guard: NonceGuard,
    data: Data<'_>,
    db: &State<GrapevineDB>,
) -> Result<Status, Status> {
    // stream in data
    // todo: implement FromData trait on NewPhraseRequest
    let mut buffer = Vec::new();
    let mut stream = data.open(2.mebibytes()); // Adjust size limit as needed
    if let Err(e) = stream.read_to_end(&mut buffer).await {
        return Err(Status::BadRequest);
    }
    let request = match bincode::deserialize::<NewPhraseRequest>(&buffer) {
        Ok(req) => req,
        Err(e) => return Err(Status::BadRequest),
    };
    let decompressed_proof = decompress_proof(&request.proof);
    // verify the proof
    let public_params = use_public_params().unwrap();
    let verify_res = verify_nova_proof(&decompressed_proof, &public_params, 2);
    let (phrase_hash, auth_hash) = match verify_res {
        Ok(res) => {
            let phrase_hash = res.0[1];
            let auth_hash = res.0[2];
            // todo: use request guard to check username against proven username
            (phrase_hash.to_bytes(), auth_hash.to_bytes())
        }
        Err(e) => {
            println!("Proof verification failed: {:?}", e);
            return Err(Status::BadRequest);
        }
    };
    // get user doc
    let user = db.get_user(request.username.clone()).await.unwrap();
    // build DegreeProof model
    let proof_doc = DegreeProof {
        id: None,
        phrase_hash: Some(phrase_hash),
        auth_hash: Some(auth_hash),
        user: Some(user.id.unwrap()),
        degree: Some(1),
        proof: Some(request.proof.clone()),
        preceding: None,
        proceeding: Some(vec![]),
    };

    match db.add_proof(&user.id.unwrap(), &proof_doc).await {
        Ok(_) => Ok(Status::Created),
        Err(e) => Err(Status::NotImplemented),
    }
}

/**
 * Build from a previous degree of connection proof and add it to the database
 *
 * @param data - binary serialized DegreeProofRequest containing:
 *             * username: the username of the user adding a proof of degree of connection
 *             * proof: the gzip-compressed fold proof
 *             * previous: the stringified OID of the previous proof to continue IVC from
 *             * degree: the separation degree of the given proof
 * @return status:
 *             * 201 if successful proof update
 *             * 400 if proof verification failed, deserialization fails, or proof decompression
 *               fails
 *             * 401 if signature mismatch or nonce mismatch
 *             * 404 if user or previous proof not found not found
 *             * 500 if db fails or other unknown issue
 */
#[post("/phrase/continue", data = "<data>")]
pub async fn degree_proof(data: Data<'_>, db: &State<GrapevineDB>) -> Result<Status, Status> {
    // stream in data
    // todo: implement FromData trait on NewPhraseRequest
    let mut buffer = Vec::new();
    let mut stream = data.open(2.mebibytes()); // Adjust size limit as needed
    if let Err(_) = stream.read_to_end(&mut buffer).await {
        return Err(Status::BadRequest);
    }
    let request = match bincode::deserialize::<DegreeProofRequest>(&buffer) {
        Ok(req) => req,
        Err(_) => return Err(Status::BadRequest),
    };
    let decompressed_proof = decompress_proof(&request.proof);
    // verify the proof
    let public_params = use_public_params().unwrap();
    let verify_res = verify_nova_proof(
        &decompressed_proof,
        &public_params,
        (request.degree * 2) as usize,
    );
    let (phrase_hash, auth_hash) = match verify_res {
        Ok(res) => {
            let phrase_hash = res.0[1];
            let auth_hash = res.0[2];
            (phrase_hash.to_bytes(), auth_hash.to_bytes())
        }
        Err(e) => {
            println!("Proof verification failed: {:?}", e);
            return Err(Status::BadRequest);
        }
    };
    // get user doc
    let user = db.get_user(request.username.clone()).await.unwrap();
    // @TODO: needs to delete a previous proof by same user on same phrase hash if exists, including removing from last proof's previous field
    // build DegreeProof struct
    let proof_doc = DegreeProof {
        id: None,
        phrase_hash: Some(phrase_hash),
        auth_hash: Some(auth_hash),
        user: Some(user.id.unwrap()),
        degree: Some(request.degree),
        proof: Some(request.proof.clone()),
        preceding: Some(ObjectId::from_str(&request.previous).unwrap()),
        proceeding: Some(vec![]),
    };

    // add proof to db and update references
    match db.add_proof(&user.id.unwrap(), &proof_doc).await {
        Ok(_) => Ok(Status::Created),
        Err(e) => Err(Status::NotImplemented),
    }
}

/// GET REQUESTS ///

/**
 * Return a list of all available (new) degree proofs from existing connections that a user can
 * build from
 *
 * @param username - the username to look up the available proofs for
 * @return - a vector of stringified OIDs of available proofs to use with get_proof_with_params
 *           route (empty if none)
 * @return status:
 *         - 200 if successful retrieval
 *         - 401 if signature mismatch or nonce mismatch
 *         - 404 if user not found
 *         - 500 if db fails or other unknown issue
 */
#[get("/proof/<username>/available")]
pub async fn get_available_proofs(
    username: String,
    db: &State<GrapevineDB>,
) -> Result<Json<Vec<String>>, Status> {
    Ok(Json(db.find_available_degrees(username).await))
}

/**
 * Returns all the information needed to construct a proof of degree of separation from a given user
 *
 * @param oid - the ObjectID of the proof to retrieve
 * @param username - the username to retrieve encrypted auth secret for when proving relationship
 * @return - a ProvingData struct containing:
 *         * degree: the separation degree of the returned proof
 *         * proof: the gzip-compressed fold proof
 *         * username: the username of the proof creator
 *         * ephemeral_key: the ephemeral pubkey that can be combined with the requesting user's
 *           private key to derive returned proof creator's auth secret decryption key
 *         * ciphertext: the encrypted auth secret
 * @return status:
 *         - 200 if successful retrieval
 *         - 401 if signature mismatch or nonce mismatch
 *         - 404 if username or proof not found
 *         - 500 if db fails or other unknown issue
 */
#[get("/proof/<oid>/params/<username>")]
pub async fn get_proof_with_params(
    oid: String,
    username: String,
    db: &State<GrapevineDB>,
) -> Result<Json<ProvingData>, Status> {
    let oid = ObjectId::from_str(&oid).unwrap();
    match db.get_proof_and_data(username, oid).await {
        Some(data) => Ok(Json(data)),
        None => Err(Status::NotFound),
    }
}