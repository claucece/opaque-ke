// Copyright (c) Facebook, Inc. and its affiliates.
//
// This source code is licensed under both the MIT license found in the
// LICENSE-MIT file in the root directory of this source tree and the Apache
// License, Version 2.0 found in the LICENSE-APACHE file in the root directory
// of this source tree.

//! Contains the messages used for OPAQUE

use crate::{
    ciphersuite::CipherSuite,
    envelope::Envelope,
    errors::{
        utils::{check_slice_size, check_slice_size_atleast},
        ProtocolError,
    },
    key_exchange::{
        group::KeGroup,
        traits::{FromBytes, KeyExchange, ToBytes},
    },
    keypair::{KeyPair, PublicKey, SecretKey},
    opaque::ServerSetup,
};
use alloc::vec::Vec;
use digest::Digest;
use generic_array::{typenum::Unsigned, GenericArray};
use rand::{CryptoRng, RngCore};
use voprf::group::Group;

////////////////////////////
// High-level API Structs //
// ====================== //
////////////////////////////

/// The message sent by the client to the server, to initiate registration
pub struct RegistrationRequest<CS: CipherSuite> {
    /// blinded password information
    pub(crate) blinded_element: voprf::BlindedElement<CS::OprfGroup, CS::Hash>,
}

/// The answer sent by the server to the user, upon reception of the
/// registration attempt
pub struct RegistrationResponse<CS: CipherSuite> {
    /// The server's oprf output
    pub(crate) evaluation_element: voprf::EvaluationElement<CS::OprfGroup, CS::Hash>,
    /// Server's static public key
    pub(crate) server_s_pk: PublicKey<CS::KeGroup>,
}

/// The final message from the client, containing sealed cryptographic
/// identifiers
pub struct RegistrationUpload<CS: CipherSuite> {
    /// The "envelope" generated by the user, containing sealed
    /// cryptographic identifiers
    pub(crate) envelope: Envelope<CS>,
    /// The masking key used to mask the envelope
    pub(crate) masking_key: GenericArray<u8, <CS::Hash as Digest>::OutputSize>,
    /// The user's public key
    pub(crate) client_s_pk: PublicKey<CS::KeGroup>,
}

/// The message sent by the user to the server, to initiate registration
pub struct CredentialRequest<CS: CipherSuite> {
    pub(crate) blinded_element: voprf::BlindedElement<CS::OprfGroup, CS::Hash>,
    pub(crate) ke1_message: <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE1Message,
}

/// The answer sent by the server to the user, upon reception of the
/// login attempt
pub struct CredentialResponse<CS: CipherSuite> {
    /// the server's oprf output
    pub(crate) evaluation_element: voprf::EvaluationElement<CS::OprfGroup, CS::Hash>,
    pub(crate) masking_nonce: Vec<u8>,
    pub(crate) masked_response: Vec<u8>,
    pub(crate) ke2_message: <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE2Message,
}

/// The answer sent by the client to the server, upon reception of the
/// sealed envelope
pub struct CredentialFinalization<CS: CipherSuite> {
    pub(crate) ke3_message: <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE3Message,
}

////////////////////////////////
// High-level Implementations //
// ========================== //
////////////////////////////////

impl<CS: CipherSuite> RegistrationRequest<CS> {
    /// Only used for testing purposes
    #[cfg(test)]
    pub fn get_blinded_element_for_testing(
        &self,
    ) -> voprf::BlindedElement<CS::OprfGroup, CS::Hash> {
        self.blinded_element.clone()
    }

    /// Serialization into bytes
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(self.blinded_element.serialize())
    }

    /// Deserialization from bytes
    pub fn deserialize(input: &[u8]) -> Result<Self, ProtocolError> {
        Ok(Self {
            blinded_element: voprf::BlindedElement::deserialize(input)?,
        })
    }
}

impl<CS: CipherSuite> RegistrationResponse<CS> {
    /// Serialization into bytes
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok([
            self.evaluation_element.serialize(),
            self.server_s_pk.to_vec(),
        ]
        .concat())
    }

    /// Deserialization from bytes
    pub fn deserialize(input: &[u8]) -> Result<Self, ProtocolError> {
        let elem_len = <CS::OprfGroup as Group>::ElemLen::USIZE;
        let key_len = <CS::KeGroup as KeGroup>::PkLen::USIZE;
        let checked_slice =
            check_slice_size(input, elem_len + key_len, "registration_response_bytes")?;

        // Ensure that public key is valid
        let server_s_pk = KeyPair::<CS::KeGroup>::check_public_key(PublicKey::from_bytes(
            &checked_slice[elem_len..],
        )?)?;

        Ok(Self {
            evaluation_element: voprf::EvaluationElement::deserialize(&checked_slice[..elem_len])?,
            server_s_pk,
        })
    }

    #[cfg(test)]
    /// Only used for tests, where we can set the beta value to test for the reflection
    /// error case
    pub fn set_evaluation_element_for_testing(&self, beta: CS::OprfGroup) -> Self {
        Self {
            evaluation_element: voprf::EvaluationElement::from_value_unchecked(beta),
            server_s_pk: self.server_s_pk.clone(),
        }
    }
}

impl<CS: CipherSuite> RegistrationUpload<CS> {
    /// Serialization into bytes
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok([
            self.client_s_pk.to_arr().to_vec(),
            self.masking_key.to_vec(),
            self.envelope.serialize(),
        ]
        .concat())
    }

    /// Deserialization from bytes
    pub fn deserialize(input: &[u8]) -> Result<Self, ProtocolError> {
        let key_len = <CS::KeGroup as KeGroup>::PkLen::USIZE;
        let hash_len = <CS::Hash as Digest>::OutputSize::USIZE;
        let checked_slice =
            check_slice_size_atleast(input, key_len + hash_len, "registration_upload_bytes")?;
        let envelope = Envelope::<CS>::deserialize(&checked_slice[key_len + hash_len..])?;
        Ok(Self {
            envelope,
            masking_key: GenericArray::clone_from_slice(
                &checked_slice[key_len..key_len + hash_len],
            ),
            client_s_pk: KeyPair::<CS::KeGroup>::check_public_key(PublicKey::from_bytes(
                &checked_slice[..key_len],
            )?)?,
        })
    }

    // Creates a dummy instance used for faking a [CredentialResponse]
    pub(crate) fn dummy<R: RngCore + CryptoRng, S: SecretKey<CS::KeGroup>>(
        rng: &mut R,
        server_setup: &ServerSetup<CS, S>,
    ) -> Self {
        let mut masking_key = alloc::vec![0u8; <CS::Hash as Digest>::OutputSize::USIZE];
        rng.fill_bytes(&mut masking_key);

        Self {
            envelope: Envelope::<CS>::dummy(),
            masking_key: GenericArray::clone_from_slice(&masking_key),
            client_s_pk: server_setup.fake_keypair.public().clone(),
        }
    }
}

impl<CS: CipherSuite> CredentialRequest<CS> {
    /// Serialization into bytes
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok([
            self.blinded_element.serialize(),
            self.ke1_message.to_bytes(),
        ]
        .concat())
    }

    /// Deserialization from bytes
    pub fn deserialize(input: &[u8]) -> Result<Self, ProtocolError> {
        let elem_len = <CS::OprfGroup as Group>::ElemLen::USIZE;

        let checked_slice = check_slice_size_atleast(input, elem_len, "login_first_message_bytes")?;

        // Check that the message is actually containing an element of the
        // correct subgroup
        let blinded_element = voprf::BlindedElement::<CS::OprfGroup, CS::Hash>::deserialize(
            &checked_slice[..elem_len],
        )?;

        // Throw an error if the identity group element is encountered
        if blinded_element.value().is_identity() {
            return Err(ProtocolError::IdentityGroupElementError);
        }

        let ke1_message =
            <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE1Message::from_bytes::<CS>(
                &checked_slice[elem_len..],
            )?;

        Ok(Self {
            blinded_element,
            ke1_message,
        })
    }

    /// Only used for testing purposes
    #[cfg(test)]
    pub fn get_blinded_element_for_testing(
        &self,
    ) -> voprf::BlindedElement<CS::OprfGroup, CS::Hash> {
        self.blinded_element.clone()
    }
}

impl<CS: CipherSuite> CredentialResponse<CS> {
    /// Serialization into bytes
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok([
            Self::serialize_without_ke(
                &self.evaluation_element.value(),
                &self.masking_nonce,
                &self.masked_response,
            ),
            self.ke2_message.to_bytes(),
        ]
        .concat())
    }

    pub(crate) fn serialize_without_ke(
        beta: &CS::OprfGroup,
        masking_nonce: &[u8],
        masked_response: &[u8],
    ) -> Vec<u8> {
        [&beta.to_arr(), masking_nonce, masked_response].concat()
    }

    /// Deserialization from bytes
    pub fn deserialize(input: &[u8]) -> Result<Self, ProtocolError> {
        let elem_len = <CS::OprfGroup as Group>::ElemLen::USIZE;
        let key_len = <CS::KeGroup as KeGroup>::PkLen::USIZE;
        let nonce_len: usize = 32;
        let envelope_len = Envelope::<CS>::len();
        let masked_response_len = key_len + envelope_len;
        let ke2_message_len = CS::KeyExchange::ke2_message_size();

        let checked_slice = check_slice_size_atleast(
            input,
            elem_len + nonce_len + masked_response_len + ke2_message_len,
            "credential_response_bytes",
        )?;

        // Check that the message is actually containing an element of the
        // correct subgroup
        let beta_bytes = &checked_slice[..elem_len];
        let evaluation_element =
            voprf::EvaluationElement::<CS::OprfGroup, CS::Hash>::deserialize(beta_bytes)?;

        // Throw an error if the identity group element is encountered
        if evaluation_element.value().is_identity() {
            return Err(ProtocolError::IdentityGroupElementError);
        }

        let masking_nonce = checked_slice[elem_len..elem_len + nonce_len].to_vec();
        let masked_response = checked_slice
            [elem_len + nonce_len..elem_len + nonce_len + masked_response_len]
            .to_vec();
        let ke2_message =
            <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE2Message::from_bytes::<CS>(
                &checked_slice[elem_len + nonce_len + masked_response_len..],
            )?;

        Ok(Self {
            evaluation_element,
            masking_nonce,
            masked_response,
            ke2_message,
        })
    }

    #[cfg(test)]
    /// Only used for tests, where we can set the beta value to test for the reflection
    /// error case
    pub fn set_evaluation_element_for_testing(&self, beta: CS::OprfGroup) -> Self {
        Self {
            evaluation_element: voprf::EvaluationElement::from_value_unchecked(beta),
            masking_nonce: self.masking_nonce.clone(),
            masked_response: self.masked_response.clone(),
            ke2_message: self.ke2_message.clone(),
        }
    }
}

impl<CS: CipherSuite> CredentialFinalization<CS> {
    /// Serialization into bytes
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(self.ke3_message.to_bytes())
    }

    /// Deserialization from bytes
    pub fn deserialize(input: &[u8]) -> Result<Self, ProtocolError> {
        let ke3_message =
            <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE3Message::from_bytes::<CS>(
                input,
            )?;
        Ok(Self { ke3_message })
    }
}

///////////////////////////
// Trait Implementations //
// ===================== //
///////////////////////////

impl_clone_for!(
    struct RegistrationRequest<CS: CipherSuite>,
    [blinded_element],
);
impl_debug_eq_hash_for!(struct RegistrationRequest<CS: CipherSuite>, [blinded_element], [CS::OprfGroup, CS::Hash]);
impl_serialize_and_deserialize_for!(RegistrationRequest);

impl_clone_for!(
    struct RegistrationResponse<CS: CipherSuite>,
    [evaluation_element, server_s_pk],
);
impl_debug_eq_hash_for!(
    struct RegistrationResponse<CS: CipherSuite>,
    [evaluation_element, server_s_pk],
    [CS::OprfGroup, CS::Hash],
);
impl_serialize_and_deserialize_for!(RegistrationResponse);

impl_clone_for!(
    struct RegistrationUpload<CS: CipherSuite>,
    [envelope, masking_key, client_s_pk],
);
impl_debug_eq_hash_for!(
    struct RegistrationUpload<CS: CipherSuite>,
    [envelope, masking_key, client_s_pk],
);
impl_serialize_and_deserialize_for!(RegistrationUpload);

impl_clone_for!(
    struct CredentialRequest<CS: CipherSuite>,
    [blinded_element, ke1_message],
);
impl_debug_eq_hash_for!(
    struct CredentialRequest<CS: CipherSuite>,
    [blinded_element, ke1_message],
    [
        CS::OprfGroup,
        <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE1Message
    ],
);
impl_serialize_and_deserialize_for!(CredentialRequest);

impl_clone_for!(
    struct CredentialResponse<CS: CipherSuite>,
    [evaluation_element, masking_nonce, masked_response, ke2_message],
);
impl_debug_eq_hash_for!(
    struct CredentialResponse<CS: CipherSuite>,
    [evaluation_element, masking_nonce, masked_response, ke2_message],
    [
        CS::OprfGroup,
        <CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE2Message,
    ],
);
impl_serialize_and_deserialize_for!(CredentialResponse);

impl_clone_for!(struct CredentialFinalization<CS: CipherSuite>, [ke3_message]);
impl_debug_eq_hash_for!(
    struct CredentialFinalization<CS: CipherSuite>,
    [ke3_message],
    [<CS::KeyExchange as KeyExchange<CS::Hash, CS::KeGroup>>::KE3Message],
);
impl_serialize_and_deserialize_for!(CredentialFinalization);
