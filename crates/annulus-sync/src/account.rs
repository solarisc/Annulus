//! The client side of OPAQUE registration and login (ADR-0008). The password and the
//! Argon2id work never leave this computer; the server sees only OPAQUE messages.

use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, Identifiers, RegistrationResponse,
};
use opaque_rand::rngs::OsRng;
use annulus_proto::{
    LoginFinish, LoginStart, LoginStartResponse, RegisterFinish, RegisterStart,
    RegisterStartResponse, Session, Suite,
};

use crate::engine::SyncError;

/// The login half of the server, as the client sees it.
pub trait Auth {
    /// Registration step 1.
    ///
    /// # Errors
    /// Unreachable, or refused (signups closed, name taken).
    fn register_start(
        &mut self,
        request: &RegisterStart,
    ) -> Result<RegisterStartResponse, SyncError>;
    /// Registration step 2.
    ///
    /// # Errors
    /// As [`Auth::register_start`].
    fn register_finish(&mut self, request: &RegisterFinish) -> Result<Session, SyncError>;
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

/// Creates `account` with `password` and signs this device in. `wrapped_keys` is the
/// vault key file, stored on the server so other devices can unwrap the same account key.
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
    let start =
        ClientRegistration::<Suite>::start(&mut OsRng, password.as_bytes()).map_err(protocol)?;
    let response = server.register_start(&RegisterStart {
        account: account.to_owned(),
        request: start.message.serialize().to_vec(),
    })?;
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
    server.register_finish(&RegisterFinish {
        account: account.to_owned(),
        upload: finish.message.serialize().to_vec(),
        wrapped_keys,
        device: device.to_owned(),
    })
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
