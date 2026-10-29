//! The client side of OPAQUE registration and login (ADR-0008). The password and the
//! Argon2id work never leave this computer; the server sees only OPAQUE messages.

use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, Identifiers, RegistrationResponse,
};
use opaque_rand::rngs::OsRng;
use annulus_proto::{
    LoginFinish, LoginStart, LoginStartResponse, PasswordBegin, PasswordChange, RegisterFinish,
    RegisterStart, RegisterStartResponse, Session, Suite,
};

use crate::engine::SyncError;

/// The login half of the server, as the client sees it.
pub trait Auth {
    /// Registration step 1, with the `invite` code a server with closed sign-ups needs.
    ///
    /// # Errors
    /// Unreachable, or refused (signups closed and no valid invite, name taken).
    fn register_start(
        &mut self,
        request: &RegisterStart,
        invite: Option<&str>,
    ) -> Result<RegisterStartResponse, SyncError>;
    /// Registration step 2, with the same `invite`, which it uses up.
    ///
    /// # Errors
    /// As [`Auth::register_start`].
    fn register_finish(
        &mut self,
        request: &RegisterFinish,
        invite: Option<&str>,
    ) -> Result<Session, SyncError>;
    /// Login step 1.
    ///
    /// # Errors
    /// Unreachable, or throttled.
    fn login_start(&mut self, request: &LoginStart) -> Result<LoginStartResponse, SyncError>;
    /// Login step 2.
    ///
    /// # Errors
    /// Unreachable, or the login failed.
    fn login_finish(&mut self, request: &LoginFinish) -> Result<Session, SyncError>;
}

/// The Argon2id cost OPAQUE stretches the password with: 64 MiB, 3 passes, one lane, the
/// same as the vault (ADR-0007).
#[must_use]
pub fn default_ksf() -> Argon2<'static> {
    let params = Params::new(64 * 1024, 3, 1, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn protocol(_: opaque_ke::errors::ProtocolError) -> SyncError {
    SyncError::Transport("the server's login messages were not valid".into())
}

/// Creates `account` with `password` and signs this device in, on a server with open
/// sign-ups. `wrapped_keys` is the vault key file, stored on the server so other devices
/// can unwrap the same account key.
///
/// # Errors
/// The server refused or could not be reached.
pub fn register(
    server: &mut impl Auth,
    ksf: &Argon2<'static>,
    account: &str,
    password: &str,
    wrapped_keys: Vec<u8>,
    device: &str,
) -> Result<Session, SyncError> {
    register_with_invite(server, ksf, account, password, wrapped_keys, device, None)
}

/// As [`register`], with the single-use `invite` code (from `annulus-server invite`) that
/// a server with closed sign-ups needs; `None` or empty sends none.
///
/// # Errors
/// The server refused (no valid invite, name taken) or could not be reached.
pub fn register_with_invite(
    server: &mut impl Auth,
    ksf: &Argon2<'static>,
    account: &str,
    password: &str,
    wrapped_keys: Vec<u8>,
    device: &str,
    invite: Option<&str>,
) -> Result<Session, SyncError> {
    let invite = invite.map(str::trim).filter(|code| !code.is_empty());
    let start =
        ClientRegistration::<Suite>::start(&mut OsRng, password.as_bytes()).map_err(protocol)?;
    let response = server.register_start(
        &RegisterStart {
            account: account.to_owned(),
            request: start.message.serialize().to_vec(),
        },
        invite,
    )?;
    let response = RegistrationResponse::deserialize(&response.response).map_err(protocol)?;
    let finish = start
        .state
        .finish(
            &mut OsRng,
            password.as_bytes(),
            response,
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(ksf)),
        )
        .map_err(protocol)?;
    server.register_finish(
        &RegisterFinish {
            account: account.to_owned(),
            upload: finish.message.serialize().to_vec(),
            wrapped_keys,
            device: device.to_owned(),
        },
        invite,
    )
}

/// Signs this device in to `account`. A wrong password is found here, before anything is
/// sent that the server could test guesses against.
///
/// # Errors
/// Wrong account or password, throttled, or unreachable.
pub fn login(
    server: &mut impl Auth,
    ksf: &Argon2<'static>,
    account: &str,
    password: &str,
    device: &str,
) -> Result<Session, SyncError> {
    let start = ClientLogin::<Suite>::start(&mut OsRng, password.as_bytes()).map_err(protocol)?;
    let response = server.login_start(&LoginStart {
        account: account.to_owned(),
        request: start.message.serialize().to_vec(),
    })?;
    let credential = CredentialResponse::deserialize(&response.response).map_err(protocol)?;
    let finish = start
        .state
        .finish(
            &mut OsRng,
            password.as_bytes(),
            credential,
            ClientLoginFinishParameters::new(None, Identifiers::default(), Some(ksf)),
        )
        .map_err(|_| SyncError::Transport("wrong account name or password".into()))?;
    server.login_finish(&LoginFinish {
        login: response.login,
        finalization: finish.message.serialize().to_vec(),
        device: device.to_owned(),
    })
}

/// Re-registers the signed-in account with `new_password` and the re-wrapped key file
/// `wrapped_keys`, proving `current_password` with a fresh login in the same exchange (the
/// device token alone is not enough); the server signs every other device out.
///
/// # Errors
/// The current password is wrong, throttled, unreachable or signed out.
pub fn change_password(
    server: &mut crate::http::HttpServer,
    ksf: &Argon2<'static>,
    current_password: &str,
    new_password: &str,
    wrapped_keys: Vec<u8>,
) -> Result<(), SyncError> {
    let login =
        ClientLogin::<Suite>::start(&mut OsRng, current_password.as_bytes()).map_err(protocol)?;
    let registration = ClientRegistration::<Suite>::start(&mut OsRng, new_password.as_bytes())
        .map_err(protocol)?;
    let response = server.password_begin(&PasswordBegin {
        login: login.message.serialize().to_vec(),
        registration: registration.message.serialize().to_vec(),
    })?;
    let credential = CredentialResponse::deserialize(&response.credential).map_err(protocol)?;
    let proof = login
        .state
        .finish(
            &mut OsRng,
            current_password.as_bytes(),
            credential,
            ClientLoginFinishParameters::new(None, Identifiers::default(), Some(ksf)),
        )
        .map_err(|_| SyncError::Transport("the current password is wrong".into()))?;
    let response_to_new =
        RegistrationResponse::deserialize(&response.registration).map_err(protocol)?;
    let upload = registration
        .state
        .finish(
            &mut OsRng,
            new_password.as_bytes(),
            response_to_new,
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(ksf)),
        )
        .map_err(protocol)?;
    server.password_change(&PasswordChange {
        login: response.login,
        finalization: proof.message.serialize().to_vec(),
        upload: upload.message.serialize().to_vec(),
        wrapped_keys,
    })
}
