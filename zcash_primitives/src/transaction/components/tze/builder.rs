//! Types and functions for building TZE transaction components
#![cfg(feature = "zfuture")]

use std::fmt;

use crate::{
    extensions::transparent::{self as tze, ToPayload},
    transaction::components::{
        amount::Amount,
        tze::{Bundle, OutPoint, TzeIn, TzeOut, Unauthorized},
    },
};

#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidAmount,
    WitnessModeMismatch(u32, u32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::InvalidAmount => write!(f, "Invalid amount"),
            Error::WitnessModeMismatch(expected, actual) =>
                write!(f, "TZE witness builder returned a mode that did not match the mode with which the input was initially constructed: expected = {:?}, actual = {:?}", expected, actual),
        }
    }
}

#[allow(clippy::type_complexity)]
struct TzeSigner<'a, BuildCtx> {
    prevout: TzeOut,
    builder: Box<dyn FnOnce(&BuildCtx) -> Result<(u32, Vec<u8>), Error> + 'a>,
}

pub struct TzeBuilder<'a, BuildCtx> {
    signers: Vec<TzeSigner<'a, BuildCtx>>,
    vin: Vec<TzeIn<Unauthorized>>,
    vout: Vec<TzeOut>,
}

impl<'a, BuildCtx> TzeBuilder<'a, BuildCtx> {
    pub fn empty() -> Self {
        TzeBuilder {
            signers: vec![],
            vin: vec![],
            vout: vec![],
        }
    }

    pub fn add_input<WBuilder, W: ToPayload>(
        &mut self,
        extension_id: u32,
        mode: u32,
        (outpoint, prevout): (OutPoint, TzeOut),
        witness_builder: WBuilder,
    ) where
        WBuilder: 'a + FnOnce(&BuildCtx) -> Result<W, Error>,
    {
        self.vin.push(TzeIn::new(outpoint, extension_id, mode));
        self.signers.push(TzeSigner {
            prevout,
            builder: Box::new(move |ctx| witness_builder(&ctx).map(|x| x.to_payload())),
        });
    }

    pub fn add_output<G: ToPayload>(
        &mut self,
        extension_id: u32,
        value: Amount,
        guarded_by: &G,
    ) -> Result<(), Error> {
        if value.is_negative() {
            return Err(Error::InvalidAmount);
        }

        let (mode, payload) = guarded_by.to_payload();
        self.vout.push(TzeOut {
            value,
            precondition: tze::Precondition {
                extension_id,
                mode,
                payload,
            },
        });

        Ok(())
    }

    pub fn value_balance(&self) -> Option<Amount> {
        self.signers
            .iter()
            .map(|s| s.prevout.value)
            .sum::<Option<Amount>>()?
            - self
                .vout
                .iter()
                .map(|tzo| tzo.value)
                .sum::<Option<Amount>>()?
    }

    pub fn build(&self) -> Option<Bundle<Unauthorized>> {
        if self.vin.is_empty() && self.vout.is_empty() {
            None
        } else {
            Some(Bundle {
                vin: self.vin.clone(),
                vout: self.vout.clone(),
            })
        }
    }

    pub fn create_witnesses(self, mtx: &BuildCtx) -> Result<Option<Vec<tze::AuthData>>, Error> {
        // Create TZE input witnesses
        if self.vin.is_empty() && self.vout.is_empty() {
            Ok(None)
        } else {
            // Create TZE input witnesses
            let payloads = self
                .signers
                .into_iter()
                .zip(self.vin.into_iter())
                .into_iter()
                .map(|(signer, tzein)| {
                    // The witness builder function should have cached/closed over whatever data was
                    // necessary for the witness to commit to at the time it was added to the
                    // transaction builder; here, it then computes those commitments.
                    let (mode, payload) = (signer.builder)(&mtx)?;
                    let input_mode = tzein.witness.mode;
                    if mode != input_mode {
                        return Err(Error::WitnessModeMismatch(input_mode, mode));
                    }

                    Ok(tze::AuthData(payload))
                })
                .collect::<Result<Vec<_>, Error>>()?;

            Ok(Some(payloads))
        }
    }
}
