use crate::catchers::{ErrorMessage, GrapevineResponse};
use crate::guards::AuthenticatedUser;
use crate::mongo::GrapevineDB;
use babyjubjub_rs::{decompress_point, decompress_signature, verify};
use grapevine_common::errors::GrapevineServerError;
use grapevine_common::http::requests::GetNonceRequest;
use grapevine_common::http::{requests::CreateUserRequest, responses::DegreeData};
use grapevine_common::utils::convert_username_to_fr;
use grapevine_common::MAX_USERNAME_CHARS;
use grapevine_common::{
    http::requests::NewRelationshipRequest,
    models::{relationship::Relationship, user::User},
};
use rocket::State;

use num_bigint::{BigInt, Sign};
use rocket::http::Status;
use rocket::serde::json::Json;

/// POST REQUESTS ///

/**
 * Create a new user of the grapevine service
 *
 * @param data - the CreateUserRequest containing:
 *             * username: the username for the new user
 *             * pubkey: the public key used to authZ/authN and deriving AES encryption keys
 *             * signature: the signature over the username by pubkey
 * @return status:
 *             * 201 if success
 *             * 400 if username length exceeds 30 characters, username is not valid ASCII,
 *               invalid signature over username by pubkey, or issues deserializing request
 *             * 409 if username || pubkey are already in use by another user
 *             * 500 if db fails or other unknown issue
 */
#[post("/create", format = "json", data = "<request>")]
pub async fn create_user(
    request: Json<CreateUserRequest>,
    db: &State<GrapevineDB>,
) -> Result<GrapevineResponse, GrapevineResponse> {
    // check username length is valid
    if request.username.len() > MAX_USERNAME_CHARS {
        return Err(GrapevineResponse::BadRequest(ErrorMessage(
            Some(GrapevineServerError::UsernameTooLong(
                request.username.clone(),
            )),
            None,
        )));
    };
    // check request is ascii
    if !request.username.is_ascii() {
        return Err(GrapevineResponse::BadRequest(ErrorMessage(
            Some(GrapevineServerError::UsernameNotAscii(
                request.username.clone(),
            )),
            None,
        )));
    };
    // check the validity of the signature over the username
    let message = BigInt::from_bytes_le(
        Sign::Plus,
        &convert_username_to_fr(&request.username).unwrap()[..],
    );
    let pubkey_decompressed = decompress_point(request.pubkey).unwrap();
    let signature_decompressed = decompress_signature(&request.signature).unwrap();
    match verify(pubkey_decompressed, signature_decompressed, message) {
        true => (),
        false => {
            return Err(GrapevineResponse::BadRequest(ErrorMessage(
                Some(GrapevineServerError::Signature(String::from(
                    "Could not verify user creation signature",
                ))),
                None,
            )));
        }
    };
    // check that the username or pubkey are not already used
    match db
        .check_creation_params(&request.username, &request.pubkey)
        .await
    {
        Ok(found) => match found {
            [true, true] => {
                return Err(GrapevineResponse::Conflict(ErrorMessage(
                    Some(GrapevineServerError::UserExists(request.username.clone())),
                    None,
                )));
            }
            [true, false] => {
                return Err(GrapevineResponse::Conflict(ErrorMessage(
                    Some(GrapevineServerError::UsernameExists(
                        request.username.clone(),
                    )),
                    None,
                )));
            }
            [false, true] => {
                return Err(GrapevineResponse::Conflict(ErrorMessage(
                    Some(GrapevineServerError::PubkeyExists(format!(
                        "0x{}",
                        hex::encode(request.pubkey.clone())
                    ))),
                    None,
                )));
            }
            _ => (),
        },
        Err(e) => {
            return Err(GrapevineResponse::InternalError(ErrorMessage(
                Some(e),
                None,
            )))
        }
    };
    // create the new user in the database
    let user = User {
        id: None,
        nonce: Some(0),
        username: Some(request.username.clone()),
        pubkey: Some(request.pubkey.clone()),
        relationships: Some(vec![]),
        degree_proofs: Some(vec![]),
    };
    match db.create_user(user).await {
        Ok(_) => Ok(GrapevineResponse::Created(
            "User succefully created".to_string(),
        )),
        Err(e) => Err(GrapevineResponse::InternalError(ErrorMessage(
            Some(e),
            None,
        ))),
    }
}

/**
 * Add a unidirectional relationship allowing the target to prove connection to the sender
 * @notice: it would be nice to have a proof of correct encryption for the ciphertext
 *
 * @param data - the NewRelationshipRequest containing:
 *             * from: the username of the sender
 *             * to: the username of the recipient
 *             * ephemeral_key: the ephemeral pubkey that target can combine with their private
 *               key to derive AES key needed to decrypt auth secret
 *             * ciphertext: the encrypted auth secret
 * @return status:
 *            * 201 if success
 *            * 400 if from == to or issues deserializing request
 *            * 401 if signanture or nonce mismatch for sender
 *            * 404 if from or to user does not exist
 *            * 409 if relationship already exists
 */
#[post("/relationship", format = "json", data = "<request>")]
pub async fn add_relationship(
    user: AuthenticatedUser,
    request: Json<NewRelationshipRequest>,
    db: &State<GrapevineDB>,
) -> Result<Status, GrapevineResponse> {
    // ensure from != to
    if &user.0 == &request.to {
        return Err(GrapevineResponse::BadRequest(ErrorMessage(
            Some(GrapevineServerError::RelationshipSenderIsTarget),
            None,
        )));
    }

    let sender = db.get_user(&user.0).await.unwrap();
    // would be nice to have a zk proof of correct encryption to recipient...
    let recipient = match db.get_user(&request.to).await {
        Some(user) => user.id.unwrap(),
        None => {
            return Err(GrapevineResponse::NotFound(String::from(
                "Recipient does not exist.".to_string(),
            )));
        }
    };
    // add relationship doc and push to recipient array
    let relationship_doc = Relationship {
        id: None,
        sender: Some(sender.id.unwrap()),
        recipient: Some(recipient),
        ephemeral_key: Some(request.ephemeral_key.clone()),
        ciphertext: Some(request.ciphertext.clone()),
    };

    match db.add_relationship(&relationship_doc).await {
        Ok(_) => Ok(Status::Created),
        Err(e) => {
            println!("Error adding relationship: {:?}", e);
            Err(GrapevineResponse::InternalError(ErrorMessage(
                Some(GrapevineServerError::MongoError(String::from(
                    "Failed to add relationship to db",
                ))),
                None,
            )))
        }
    }
}

/// GET REQUESTS ///

/**
 * @todo: remove / replace with get nonce
 */
#[get("/<username>")]
pub async fn get_user(
    username: String,
    db: &State<GrapevineDB>,
) -> Result<Json<User>, GrapevineResponse> {
    match db.get_user(&username).await {
        Some(user) => Ok(Json(user)),
        None => Err(GrapevineResponse::NotFound(format!(
            "User {} does not exist.",
            username
        ))),
    }
}

#[post("/nonce", format = "json", data = "<request>")]
pub async fn get_nonce(
    request: Json<GetNonceRequest>,
    db: &State<GrapevineDB>,
) -> Result<String, GrapevineResponse> {
    // get pubkey & nonce for user
    let (nonce, pubkey) = match db.get_nonce(&request.username).await {
        Some((nonce, pubkey)) => (nonce, pubkey),
        None => {
            return Err(GrapevineResponse::NotFound(String::from(
                "User not does not exist.",
            )))
        }
    };
    // check the validity of the signature over the username
    let message = BigInt::from_bytes_le(
        Sign::Plus,
        &convert_username_to_fr(&request.username).unwrap()[..],
    );
    let pubkey_decompressed = decompress_point(pubkey).unwrap();
    let signature_decompressed = decompress_signature(&request.signature).unwrap();
    match verify(pubkey_decompressed, signature_decompressed, message) {
        true => (),
        false => {
            return Err(GrapevineResponse::BadRequest(ErrorMessage(
                Some(GrapevineServerError::Signature(String::from(
                    "Could not verify nonce recovery signature",
                ))),
                None,
            )));
        }
    };
    // return the stringified nonce
    Ok(nonce.to_string())
}

/**
 * Return the public key of a given user
 *
 * @param username - the username to look up the public key for
 * @return - the public key of the user
 * @return status:
 *            * 200 if success
 *            * 404 if user not found
 *            * 500 if db fails or other unknown issue
 */
#[get("/<username>/pubkey")]
pub async fn get_pubkey(
    username: String,
    db: &State<GrapevineDB>,
) -> Result<String, GrapevineResponse> {
    match db.get_pubkey(username).await {
        Some(pubkey) => Ok(hex::encode(pubkey)),
        None => Err(GrapevineResponse::NotFound(String::from(
            "User not does not exist.",
        ))),
    }
}

/**
 * Return a list of all available (new) degree proofs from existing connections that a user can
 * build from (empty if none)
 *
 * @param username - the username to look up the available proofs for
 * @return - a vector of DegreeData structs containing:
 *             * oid: the ObjectID of the proof to build from
 *             * relation: the separation degree of the proof
 *             * phrase_hash: the poseidon hash of the original phrase at the start of the chain
 * @return status:
 *            * 200 if success
 *            * 401 if signature mismatch or nonce mismatch
 *            * 404 if user not found
 *            * 500 if db fails or other unknown issue
 */
#[get("/degrees")]
pub async fn get_all_degrees(
    user: AuthenticatedUser,
    db: &State<GrapevineDB>,
) -> Result<Json<Vec<DegreeData>>, GrapevineResponse> {
    println!("in");
    match db.get_all_degrees(user.0).await {
        Some(proofs) => Ok(Json(proofs)),
        None => Err(GrapevineResponse::InternalError(ErrorMessage(
            Some(GrapevineServerError::MongoError(String::from(
                "Error retrieving degrees in db",
            ))),
            None,
        ))),
    }
}

// /**
//  * Return a list of the usernames of all direct connections by a given user
//  *
//  * @param username - the username to look up relationships for
//  * @return - a vector of stringified usernames of direct connections (empty if none found)
//  * @return status:
//  *            * 200 if success
//  *            * 401 if signature mismatch or nonce mismatch for requested user
//  *            * 404 if user not found
//  *            * 500 if db fails or other unknown issue
//  */
// pub async fn get_relationships(
//     username: String,
//     db: &State<GrapevineDB>,
// ) -> Result<Json<Vec<String>>, Status> {
//     todo!("implement get_relationships")
// }
