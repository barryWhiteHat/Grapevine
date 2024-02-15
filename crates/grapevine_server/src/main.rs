#[macro_use]
extern crate rocket;
use catchers::{bad_request, not_found, unauthorized, GrapevineResponder};
use grapevine_common::auth_secret::AuthSecretEncrypted;
use grapevine_common::http::requests::TestProofCompressionRequest;
use grapevine_common::models::user::User;
use grapevine_common::session_key::{Server, SessionKey};
use grapevine_common::utils::convert_username_to_fr;
use mongo::GrapevineDB;
use num_bigint::{BigInt, Sign};
use rocket::http::{ContentType, Header, Status};
use routes::{
    add_relationship, create_phrase, create_user, degree_proof, get_all_degrees,
    get_available_proofs, get_proof_with_params, get_pubkey, get_user,
};

use crate::guards::NonceGuard;
use mongodb::bson::doc;
use rocket::fs::{relative, FileServer};

mod catchers;
mod guards;
mod mongo;
mod routes;
mod utils;

const MONGODB_URI: &str = "mongodb://localhost:27017";
const DATABASE: &str = "grapevine";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // connect to mongodb
    let mongo = GrapevineDB::init().await;
    // Initialize logger
    tracing_subscriber::fmt::init();
    // TODO: Route formatting/ segmenting logic
    rocket::build()
        .manage(mongo)
        .mount(
            "/",
            routes![
                action,
                health,
                create_user,
                get_user,
                create_phrase,
                get_pubkey,
                add_relationship,
                get_available_proofs,
                get_proof_with_params,
                degree_proof,
                get_all_degrees
            ],
        )
        .mount("/static", FileServer::from(relative!("static")))
        .register("/", catchers![bad_request, not_found, unauthorized])
        .launch()
        .await
        .unwrap();
    Ok(())
}

#[get("/nonce-guard-test")]
async fn action(_guard: NonceGuard) -> &'static str {
    "Succesfully verified nonce"
}

#[get("/health")]
async fn health() -> &'static str {
    "Hello, world!"
}

#[cfg(test)]
mod test_rocket {
    use self::utils::{use_public_params, use_r1cs, use_wasm};

    use super::*;
    use babyjubjub_rs::PrivateKey;
    use grapevine_circuits::{nova::nova_proof, utils::compress_proof};
    use grapevine_common::{
        account::GrapevineAccount,
        auth_secret::AuthSecretEncryptedUser,
        http::requests::{CreateUserRequest, NewPhraseRequest},
        models::user,
        utils::random_fr,
    };
    use lazy_static::lazy_static;
    use rocket::{
        local::asynchronous::{Client, LocalResponse},
        request,
        serde::json::Json,
    };
    use std::sync::Mutex;

    lazy_static! {
        static ref USERS: Mutex<Vec<GrapevineAccount>> = Mutex::new(vec![]);
    }

    struct GrapevineTestContext {
        client: Client,
    }

    impl GrapevineTestContext {
        async fn init() -> Self {
            let mongo = GrapevineDB::init().await;
            let rocket = rocket::build()
                .manage(mongo)
                .mount(
                    "/",
                    routes![
                        action,
                        health,
                        create_user,
                        get_user,
                        create_phrase,
                        get_pubkey,
                        add_relationship,
                        get_available_proofs,
                        get_proof_with_params,
                    ],
                )
                .mount("/static", FileServer::from(relative!("static")))
                .register("/", catchers![bad_request, not_found, unauthorized]);

            GrapevineTestContext {
                client: Client::tracked(rocket).await.unwrap(),
            }
        }
    }

    async fn clear_user_from_db(username: String) {
        let db = GrapevineDB::init().await;
        let user = db.get_user(username.clone()).await;
        if user.is_some() {
            db.remove_user(&user.unwrap().id.unwrap()).await;
        }
    }

    /* Returns status code */
    async fn create_user_request(
        context: &GrapevineTestContext,
        request: &CreateUserRequest,
    ) -> String {
        context
            .client
            .post("/user/create")
            .header(ContentType::JSON)
            .body(serde_json::json!(request).to_string())
            .dispatch()
            .await
            .into_string()
            .await
            .unwrap()
    }

    // async fn get_user_request(context: &GrapevineTestContext, username: String) {
    //     let res:  = context
    //         .client
    //         .post(format!("/user/{}", username))
    //         .dispatch()
    //         .await
    //         .into_json()
    //         .await
    //         .unwrap();
    //     println!("{:?}", res)
    // }

    // fn check_test_env_prepared() -> bool {
    //     let users = USERS.lock().unwrap();
    //     users.get(0).is_some()
    // }

    // async fn prepare_test_env() {
    //     let mut users = USERS.lock().unwrap();
    //     let username_1 = String::from("manbearpig");
    //     // Check if user exists or not in database. If it does then remove it so that test can be performed
    //     clear_user_from_db(username_1.clone()).await;
    //     let user_1 = GrapevineTestUser {
    //         nonce: 0,
    //         private_key: new_private_key().to_vec(),
    //         username: username_1,
    //     };
    //     users.push(user_1.clone());
    //     create_user_request(
    //         user_1.username.clone(),
    //         user_1.username.clone(),
    //         PrivateKey::import(user_1.private_key).unwrap(),
    //         false,
    //     )
    //     .await;
    //     // Create user in db
    // }

    #[rocket::async_test]
    async fn test_create_user_wrong_signature() {
        // todo: CTOR for running beforeAll
        // initiate context
        let context = GrapevineTestContext::init().await;
        // generate two accounts
        let account_1 = GrapevineAccount::new(String::from("userA1"));
        let account_2 = GrapevineAccount::new(String::from("userA2"));
        // generate a signature from account 2
        let bad_sig = account_2.sign_username().compress();
        // generate a "Create User" http request from account 1
        let mut request = account_1.create_user_request();
        // set the signature for creating account 1 to be the signature of account 2
        request.signature = bad_sig;
        // check response failure
        assert_eq!(
            create_user_request(&context, &request).await,
            "Signature by pubkey does not match given message",
            "Request should fail due to mismatched msg"
        );
    }

    #[rocket::async_test]
    async fn test_username_exceeding_character_limit() {
        let context = GrapevineTestContext::init().await;

        let account = GrapevineAccount::new(String::from("userA1"));

        let mut request = account.create_user_request();

        request.username = String::from("fake_username_1234567890_abcdef");

        assert_eq!(
            create_user_request(&context, &request).await,
            format!(
                "Username {} exceeds limit of 30 characters.",
                request.username
            ),
            "Request should fail to to character length exceeded"
        );
    }

    #[rocket::async_test]
    async fn test_username_with_non_ascii_characters() {
        let context = GrapevineTestContext::init().await;

        let account = GrapevineAccount::new(String::from("fake_username_😍😌£"));

        let request = account.create_user_request();

        assert_eq!(
            create_user_request(&context, &request).await,
            "Username must only contain ascii characters.",
            "User should be created"
        );
    }

    #[rocket::async_test]
    async fn test_successful_user_creation() {
        let username = String::from("manbearpig");
        clear_user_from_db(username.clone()).await;

        let context = GrapevineTestContext::init().await;

        let account = GrapevineAccount::new(username.clone());

        let request = account.create_user_request();

        assert_eq!(
            create_user_request(&context, &request).await,
            "User succefully created",
            "User should be created"
        );

        // Check that user was stored in DB
        // get_user_request(&context, username).await;

        // Store user for later testing
        let mut users = USERS.lock().unwrap();
        users.push(account);
    }

    #[rocket::async_test]
    async fn test_nonce_guard_missing_auth_header() {
        let context = GrapevineTestContext::init().await;

        // Test no authorization header
        let res = context.client.get("/nonce-guard-test").dispatch().await;
        let message = res.into_string().await.unwrap();
        assert_eq!("Missing authorization header", message);
    }

    #[rocket::async_test]
    async fn test_nonce_guard_malformed_auth_header() {
        let context = GrapevineTestContext::init().await;

        let auth_header = Header::new("Authorization", "Missing_delimeter");
        let res = context
            .client
            .get("/nonce-guard-test")
            .header(auth_header)
            .dispatch()
            .await;
        let message = res.into_string().await.unwrap();
        assert_eq!("Malformed authorization header", message);
    }

    #[rocket::async_test]
    async fn test_nonce_guard_non_existent_user() {
        let context = GrapevineTestContext::init().await;
        let auth_header = Header::new("Authorization", "charlie-0");
        let res = context
            .client
            .get("/nonce-guard-test")
            .header(auth_header)
            .dispatch()
            .await;
        let message = res.into_string().await.unwrap();
        assert_eq!("User charlie not found", message);
    }

    #[rocket::async_test]
    async fn test_nonce_guard_successful_verification() {
        let users = USERS.lock().unwrap();
        let user = users.get(0).unwrap().clone();
        let auth_header = Header::new(
            "Authorization",
            format!("{}-{}", user.username(), user.nonce()),
        );
        let context = GrapevineTestContext::init().await;
        let res = context
            .client
            .get("/nonce-guard-test")
            .header(auth_header)
            .dispatch()
            .await;
        let message = res.into_string().await.unwrap();
        assert_eq!("Succesfully verified nonce", message);
    }

    #[rocket::async_test]
    async fn test_nonce_guard_without_nonce_increment() {
        let mut users = USERS.lock().unwrap();
        let mut user = users.get(0).unwrap().clone();
        let nonce = user.nonce();
        let auth_header = Header::new("Authorization", format!("{}-{}", user.username(), nonce));
        let context = GrapevineTestContext::init().await;
        let res = context
            .client
            .get("/nonce-guard-test")
            .header(auth_header)
            .dispatch()
            .await;
        let message = res.into_string().await.unwrap();
        assert_eq!(
            format!(
                "Incorrect nonce provided. Expected {} and received {}",
                nonce + 1,
                nonce
            ),
            message
        );
        user.increment_nonce();
        users[0] = user;
    }

    #[rocket::async_test]
    async fn test_nonce_guard_after_nonce_increment() {
        let mut users = USERS.lock().unwrap();
        let mut user = users.get(0).unwrap().clone();
        let context = GrapevineTestContext::init().await;
        let auth_header = Header::new(
            "Authorization",
            format!("{}-{}", user.username(), user.nonce()),
        );
        let res = context
            .client
            .get("/nonce-guard-test")
            .header(auth_header)
            .dispatch()
            .await;
        let message = res.into_string().await.unwrap();
        assert_eq!("Succesfully verified nonce", message);

        user.increment_nonce();
        users[0] = user;
    }

    // #############################################

    // #[rocket::async_test]
    // async fn test_duplicate_username() {
    //     let username = String::from("manbearpig");
    //     clear_user_from_db(username.clone()).await;

    //     let context = GrapevineTestContext::init().await;

    //     let account = GrapevineAccount::new(username.clone());

    //     let request = account.create_user_request();

    //     assert_eq!(
    //         create_user_request(&context, &request).await,
    //         "User succefully created",
    //         "User should be created"
    //     );
    // }

    // #[rocket::async_test]
    // async fn test_create_user() {
    //     // Test mismatched username and signed message
    //     let private_key = PrivateKey::import(new_private_key().to_vec()).unwrap();
    //     let res_code = create_user_request(
    //         String::from("fakename1"),
    //         String::from("fakename2"),
    //         private_key,
    //         false,
    //     )
    //     .await;

    //     // TODO: Replace with custom error message
    //     assert_eq!(
    //         res_code,
    //         Status::BadRequest.code,
    //         "Request should fail due to mismatched msg"
    //     );

    //     // Test username that exceeds 30 characters
    //     let username_2 = String::from("fake_username_1234567890_abcdef");
    //     let private_key_2 = PrivateKey::import(new_private_key().to_vec()).unwrap();
    //     let res_code_2 =
    //         create_user_request(username_2.clone(), username_2.clone(), private_key_2, true).await;

    //     // TODO: Replace with custom error message
    //     assert_eq!(
    //         res_code_2,
    //         Status::BadRequest.code,
    //         "Request should fail from username exceeding 30 character limit"
    //     );

    //     // Test username that contains non ascii characters
    //     let username_3 = String::from("fake_username_😍😌£");
    //     let private_key_3 = PrivateKey::import(new_private_key().to_vec()).unwrap();
    //     let res_code_3 =
    //         create_user_request(username_3.clone(), username_3.clone(), private_key_3, false).await;

    //     // TODO: Replace with custom error message
    //     assert_eq!(
    //         res_code_3,
    //         Status::BadRequest.code,
    //         "Request should fail from username containing non ascii characters"
    //     );

    //     // Successfully create user
    //     let username_4 = String::from("manbearape");
    //     let private_key_4_bytes = new_private_key().to_vec();
    //     let private_key_4 = PrivateKey::import(private_key_4_bytes.clone()).unwrap();
    //     let res_code_4 =
    //         create_user_request(username_4.clone(), username_4.clone(), private_key_4, false).await;
    //     assert_eq!(res_code_4, Status::Created.code, "Request should succeed");

    //     // TODO: Conflict errors are not returning correctly. May need to change mongo function logic

    //     // Test username that's already been stored
    //     // let username_5 = String::from("manbearpig");
    //     // let private_key_5 = PrivateKey::import(new_private_key().to_vec()).unwrap();
    //     // let res_code_5 =
    //     //     create_user_request(username_5.clone(), username_5.clone(), private_key_5, false).await;
    //     // assert_eq!(
    //     //     res_code_5,
    //     //     Status::Conflict.code,
    //     //     "Request should fail due to username already stored"
    //     // );

    //     // Test public key that's already been stored
    //     // let username_6 = String::from("manbearape");
    //     // let private_key_6 = PrivateKey::import(private_key_4_bytes.clone()).unwrap();
    //     // let res_code_6 =
    //     //     create_user_request(username_6.clone(), username_6.clone(), private_key_6, false).await;
    //     // assert_eq!(
    //     //     res_code_6,
    //     //     Status::Conflict.code,
    //     //     "Request should fail due to pubkey already stored"
    //     // );

    //     // Test public key and username that are already been stored
    //     // let username_7 = String::from("manbearpig");
    //     // let private_key_7 = PrivateKey::import(private_key_4_bytes.clone()).unwrap();
    //     // let res_code_7 =
    //     //     create_user_request(username_7.clone(), username_7.clone(), private_key_7, false).await;
    //     // assert_eq!(
    //     //     res_code_7,
    //     //     Status::Conflict.code,
    //     //     "Request should fail due to pubkey and username already stored"
    //     // );

    //     // TODO: Battle test route for more errors
    // }

    // #[rocket::async_test]
    // async fn test_create_phrase() {
    //     // Check that test environment has been prepared. If not then create users
    //     if !check_test_env_prepared() {
    //         prepare_test_env().await
    //     }

    //     // Load in user from previous tests
    //     let mut users = USERS.lock().unwrap();
    //     let mut user_1 = users.get(0).unwrap().clone();
    //     let mut nonce = user_1.nonce;
    //     let username_2 = String::from("omniman");
    //     // Test case where user does not exist
    //     let phrase = String::from("She'll be coming around the mountain when she comes");

    //     let auth_header = Header::new("Authorization", format!("{}-{}", username_2, 0));

    //     let GrapevineTestContext { client } = GrapevineTestContext::init().await;
    //     let code = client
    //         .post("/phrase/create")
    //         .header(auth_header)
    //         .body(vec![])
    //         .dispatch()
    //         .await
    //         .status()
    //         .code;

    //     // TODO: Replace code with error message
    //     assert_eq!(code, Status::NotFound.code);

    //     // Request should fail when request body cannot be parsed into NewPhraseRequest
    //     let auth_header_2 = Header::new(
    //         "Authorization",
    //         format!("{}-{}", user_1.username.clone(), nonce),
    //     );
    //     nonce += 1;
    //     let code = client
    //         .post("/phrase/create")
    //         .header(auth_header_2)
    //         .body(vec![])
    //         .dispatch()
    //         .await
    //         .status()
    //         .code;

    //     // TODO: Replace code with error message
    //     assert_eq!(code, Status::BadRequest.code);

    //     // TODO: Fix proof decompression function to account for this error
    //     // Test request with empty proof inside of body

    //     // let body = NewPhraseRequest {
    //     //     proof: vec![8],
    //     //     username: user_1.username.clone(),
    //     // };
    //     // let serialized: Vec<u8> = bincode::serialize(&body).unwrap();
    //     // let auth_header_3 = Header::new(
    //     //     "Authorization",
    //     //     format!("{}-{}", user_1.username.clone(), nonce),
    //     // );
    //     // nonce += 1;
    //     // let code = client
    //     //     .post("/phrase/create")
    //     //     .header(auth_header_3)
    //     //     .body(serialized)
    //     //     .dispatch()
    //     //     .await
    //     //     .status()
    //     //     .code;

    //     // // TODO: Replace code with error message
    //     // assert_eq!(code, Status::BadRequest.code);

    //     // Test case with valid proof

    //     let params = use_public_params().unwrap();
    //     let r1cs = use_r1cs().unwrap();
    //     let wc_path = use_wasm().unwrap();

    //     let username = String::from("omniman");
    //     clear_user_from_db(username.clone()).await;

    //     let username_vec = vec![username.clone()];
    //     let auth_secret_vec = vec![random_fr()];

    //     let proof = nova_proof(
    //         wc_path,
    //         &r1cs,
    //         &params,
    //         &phrase,
    //         &username_vec,
    //         &auth_secret_vec,
    //     )
    //     .unwrap();

    //     let compressed = compress_proof(&proof);

    //     let body = NewPhraseRequest {
    //         proof: compressed,
    //         username: user_1.username.clone(),
    //     };
    //     let serialized: Vec<u8> = bincode::serialize(&body).unwrap();
    //     let auth_header_3 = Header::new(
    //         "Authorization",
    //         format!("{}-{}", user_1.username.clone(), nonce),
    //     );
    //     nonce += 1;
    //     let code = client
    //         .post("/phrase/create")
    //         .header(auth_header_3)
    //         .body(serialized)
    //         .dispatch()
    //         .await
    //         .status()
    //         .code;

    //     assert_eq!(code, Status::Created.code);

    //     // Save updated nonce
    //     user_1.nonce = nonce;
    //     users[0] = user_1;
    // }

    #[rocket::async_test]
    async fn test_continue_phrase() {}
}
