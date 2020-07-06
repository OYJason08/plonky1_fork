use anyhow::{anyhow, bail, ensure, Result};

use crate::plonk_challenger::Challenger;
use crate::plonk_util::{halo_g, halo_s, pedersen_hash};
use crate::{AffinePoint, AffinePointTarget, Curve, Field, MsmPrecomputation, PartialWitness, Target, SECURITY_BITS};

#[derive(Debug, Clone, Copy)]
pub struct SchnorrProof<C: Curve> {
    pub r: AffinePoint<C>,
    pub z1: C::ScalarField,
    pub z2: C::ScalarField,
}

pub struct SchnorrProofTarget {
    pub r: AffinePointTarget,
    pub z1: Target,
    pub z2: Target,
}

#[derive(Debug, Clone)]
pub struct Proof<C: Curve> {
    /// A commitment to each wire polynomial.
    pub c_wires: Vec<AffinePoint<C>>,
    /// A commitment to Z, in the context of the permutation argument.
    pub c_plonk_z: AffinePoint<C>,
    /// A commitment to the quotient polynomial.
    pub c_plonk_t: Vec<AffinePoint<C>>,

    /// The opening of each polynomial at each `PublicInputGate` index.
    pub o_public_inputs: Vec<OpeningSet<C::ScalarField>>,
    /// The opening of each polynomial at `zeta`.
    pub o_local: OpeningSet<C::ScalarField>,
    /// The opening of each polynomial at `g * zeta`.
    pub o_right: OpeningSet<C::ScalarField>,
    /// The opening of each polynomial at `g^65 * zeta`.
    pub o_below: OpeningSet<C::ScalarField>,

    /// L in the Halo reduction.
    pub halo_l: Vec<AffinePoint<C>>,
    /// R in the Halo reduction.
    pub halo_r: Vec<AffinePoint<C>>,
    /// The purported value of G, i.e. <s, G>, in the context of Halo.
    pub halo_g: AffinePoint<C>,
    /// The data used in the final Schnorr protocol of the Halo opening proof.
    pub schnorr_proof: SchnorrProof<C>,
}

impl<C: Curve> Proof<C> {
    pub fn all_opening_sets(&self) -> Vec<OpeningSet<C::ScalarField>> {
        [
            self.o_public_inputs.as_slice(),
            &[
                self.o_local.clone(),
                self.o_right.clone(),
                self.o_below.clone(),
            ],
        ]
        .concat()
    }

    // Computes all challenges used in the proof verification.
    pub fn get_challenges(&self) -> Result<ProofChallenge<C>> {
        let mut challenger = Challenger::new(SECURITY_BITS);
        let error_msg = "Conversion from base to scalar field failed.";
        challenger.observe_affine_points(&self.c_wires);
        let (beta_bf, gamma_bf) = challenger.get_2_challenges();
        let beta = C::try_convert_b2s(beta_bf).map_err(|_| anyhow!(error_msg))?;
        let gamma = C::try_convert_b2s(gamma_bf).map_err(|_| anyhow!(error_msg))?;
        challenger.observe_affine_point(self.c_plonk_z);
        let alpha_bf = challenger.get_challenge();
        let alpha = C::try_convert_b2s(alpha_bf).map_err(|_| anyhow!(error_msg))?;
        challenger.observe_affine_points(&self.c_plonk_t);
        let zeta_bf = challenger.get_challenge();
        let zeta = C::try_convert_b2s(zeta_bf).map_err(|_| anyhow!(error_msg))?;
        for os in self.all_opening_sets().iter() {
            for &f in os.to_vec().iter() {
                challenger.observe_element(C::try_convert_s2b(f).map_err(|_| anyhow!(error_msg))?);
            }
        }
        let (v_bf, u_bf, u_scaling_bf) = challenger.get_3_challenges();
        let v = C::try_convert_b2s(v_bf).map_err(|_| anyhow!(error_msg))?;
        let u = C::try_convert_b2s(u_bf).map_err(|_| anyhow!(error_msg))?;
        let u_scaling = C::try_convert_b2s(u_scaling_bf).map_err(|_| anyhow!(error_msg))?;

        // Compute IPA challenges.
        let mut ipa_challenges = Vec::new();
        for i in 0..self.halo_l.len() {
            challenger.observe_affine_points(&[self.halo_l[i], self.halo_r[i]]);
            let l_challenge = challenger.get_challenge();
            ipa_challenges.push(C::try_convert_b2s(l_challenge).map_err(|_| anyhow!(error_msg))?);
        }

        // Compute challenge for Schnorr protocol.
        challenger.observe_affine_point(self.schnorr_proof.r);
        let schnorr_challenge_bf = challenger.get_challenge();
        let schnorr_challenge =
            C::try_convert_b2s(schnorr_challenge_bf).map_err(|_| anyhow!(error_msg))?;

        Ok(ProofChallenge {
            beta,
            gamma,
            alpha,
            zeta,
            v,
            u,
            u_scaling,
            ipa_challenges,
            schnorr_challenge,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProofChallenge<C: Curve> {
    pub beta: C::ScalarField,
    pub gamma: C::ScalarField,
    pub alpha: C::ScalarField,
    pub zeta: C::ScalarField,
    pub v: C::ScalarField,
    pub u: C::ScalarField,
    pub u_scaling: C::ScalarField,
    pub ipa_challenges: Vec<C::ScalarField>,
    pub schnorr_challenge: C::ScalarField,
}

#[derive(Debug, Clone)]
/// Object returned by the verifier, containing the necessary data to verify `halo_g` at a later time.
/// In particular, `halo_g = commit(g(X, ipa_challenges))` where `g` is the polynomial defined in section 3.2 of the paper.
pub struct OldProof<C: Curve> {
    pub halo_g: AffinePoint<C>,
    pub ipa_challenges: Vec<C::ScalarField>,
}

impl<C: Curve> From<Proof<C>> for OldProof<C> {
    fn from(proof: Proof<C>) -> Self {
        let ProofChallenge { ipa_challenges, .. } = proof.get_challenges().unwrap();
        Self {
            halo_g: proof.halo_g,
            ipa_challenges,
        }
    }
}

impl<C: Curve> OldProof<C> {
    /// Returns the coefficients of the Halo `g` polynomial.
    /// In particular, `commit(self.coeffs) = self.halo_g`.
    pub fn coeffs(&self) -> Vec<C::ScalarField> {
        halo_s(&self.ipa_challenges)
    }

    /// Evaluates the Halo g polynomial at a point `x`.
    pub fn evaluate_g(&self, x: C::ScalarField) -> C::ScalarField {
        halo_g(x, &self.ipa_challenges)
    }
}

#[derive(Debug, Clone)]
/// The `Target` version of `OldProof`.
pub struct OldProofTarget {
    pub halo_g: AffinePointTarget,
    pub ipa_challenges: Vec<Target>,
}

impl OldProofTarget {
    pub fn populate_witness<C: Curve>(
        &self,
        witness: &mut PartialWitness<C::BaseField>,
        values: &OldProof<C>,
    ) -> Result<()> {
        witness.set_point_target(self.halo_g, values.halo_g);
        debug_assert_eq!(self.ipa_challenges.len(), values.ipa_challenges.len());
        witness.set_targets(
            &self.ipa_challenges,
            &C::ScalarField::try_convert_all(&values.ipa_challenges)?,
        );

        Ok(())
    }
}

pub struct ProofTarget {
    /// A commitment to each wire polynomial.
    pub c_wires: Vec<AffinePointTarget>,
    /// A commitment to Z, in the context of the permutation argument.
    pub c_plonk_z: AffinePointTarget,
    /// A commitment to the quotient polynomial.
    pub c_plonk_t: Vec<AffinePointTarget>,

    /// The opening of each polynomial at each `PublicInputGate` index.
    pub o_public_inputs: Vec<OpeningSetTarget>,
    /// The opening of each polynomial at `zeta`.
    pub o_local: OpeningSetTarget,
    /// The opening of each polynomial at `g * zeta`.
    pub o_right: OpeningSetTarget,
    /// The opening of each polynomial at `g^65 * zeta`.
    pub o_below: OpeningSetTarget,

    /// L_i in the Halo reduction.
    pub halo_l_i: Vec<AffinePointTarget>,
    /// R_i in the Halo reduction.
    pub halo_r_i: Vec<AffinePointTarget>,
    /// The purported value of G, i.e. <s, G>, in the context of Halo.
    pub halo_g: AffinePointTarget,
    /// The data used in the final Schnorr protocol of the Halo opening proof.
    pub schnorr_proof: SchnorrProofTarget,
}

impl ProofTarget {
    /// `log_2(d)`, where `d` is the degree of the proof being verified.
    fn degree_pow(&self) -> usize {
        self.halo_l_i.len()
    }

    pub fn all_opening_sets(&self) -> Vec<OpeningSetTarget> {
        [
            self.o_public_inputs.as_slice(),
            &[
                self.o_local.clone(),
                self.o_right.clone(),
                self.o_below.clone(),
            ],
        ]
        .concat()
    }

    pub fn all_opening_targets(&self) -> Vec<Target> {
        let targets_2d: Vec<Vec<Target>> = self
            .all_opening_sets()
            .into_iter()
            .map(|set| set.to_vec())
            .collect();
        targets_2d.concat()
    }

    pub fn populate_witness<C: Curve>(
        &self,
        witness: &mut PartialWitness<C::BaseField>,
        values: Proof<C>,
    ) -> Result<()> {
        witness.set_point_targets(&self.c_wires, &values.c_wires);
        witness.set_point_target(self.c_plonk_z, values.c_plonk_z);
        witness.set_point_targets(&self.c_plonk_t, &values.c_plonk_t);

        debug_assert_eq!(self.o_public_inputs.len(), values.o_public_inputs.len());
        for (o_pi_targets, o_pi_values) in self.o_public_inputs.iter().zip(values.o_public_inputs) {
            o_pi_targets.populate_witness(witness, o_pi_values)?;
        }

        self.o_local.populate_witness(witness, values.o_local)?;
        self.o_right.populate_witness(witness, values.o_right)?;
        self.o_below.populate_witness(witness, values.o_below)?;

        witness.set_point_targets(&self.halo_l_i, &values.halo_l);
        witness.set_point_targets(&self.halo_r_i, &values.halo_r);
        witness.set_point_target(self.halo_g, values.halo_g);

        witness.set_point_target(self.schnorr_proof.r, values.schnorr_proof.r);
        witness.set_target(
            self.schnorr_proof.z1,
            C::ScalarField::try_convert(&values.schnorr_proof.z1)?,
        );
        witness.set_target(
            self.schnorr_proof.z2,
            C::ScalarField::try_convert(&values.schnorr_proof.z2)?,
        );

        Ok(())
    }
}

/// The opening of each Plonk polynomial at a particular point.
#[derive(Clone, Debug)]
pub struct OpeningSet<F: Field> {
    /// The purported opening of each constant polynomial.
    pub o_constants: Vec<F>,
    /// The purported opening of each S_sigma polynomial in the context of Plonk's permutation argument.
    pub o_plonk_sigmas: Vec<F>,
    /// The purported opening of each wire polynomial.
    pub o_wires: Vec<F>,
    /// The purported opening of `Z`.
    pub o_plonk_z: F,
    /// The purported opening of `t`.
    pub o_plonk_t: Vec<F>,
    /// The purported opening of some old proofs `halo_g` polynomials.
    pub o_old_proofs: Vec<F>,
}

impl<F: Field> OpeningSet<F> {
    pub fn to_vec(&self) -> Vec<F> {
        [
            self.o_constants.as_slice(),
            self.o_plonk_sigmas.as_slice(),
            self.o_wires.as_slice(),
            &[self.o_plonk_z],
            self.o_plonk_t.as_slice(),
            self.o_old_proofs.as_slice(),
        ]
        .concat()
    }
}

/// The opening of each Plonk polynomial at a particular point.
#[derive(Clone)]
pub struct OpeningSetTarget {
    /// The purported opening of each constant polynomial.
    pub o_constants: Vec<Target>,
    /// The purported opening of each S_sigma polynomial in the context of Plonk's permutation argument.
    pub o_plonk_sigmas: Vec<Target>,
    /// The purported opening of each wire polynomial.
    pub o_wires: Vec<Target>,
    /// The purported opening of `Z`.
    pub o_plonk_z: Target,
    /// The purported opening of `t`.
    pub o_plonk_t: Vec<Target>,
    /// The purported opening of some old proofs `halo_g` polynomials.
    pub o_old_proofs: Vec<Target>,
}

impl OpeningSetTarget {
    pub fn to_vec(&self) -> Vec<Target> {
        [
            self.o_constants.as_slice(),
            self.o_plonk_sigmas.as_slice(),
            self.o_wires.as_slice(),
            &[self.o_plonk_z],
            self.o_plonk_t.as_slice(),
            self.o_old_proofs.as_slice(),
        ]
        .concat()
    }

    pub fn populate_witness<InnerBF: Field, InnerSF: Field>(
        &self,
        witness: &mut PartialWitness<InnerBF>,
        values: OpeningSet<InnerSF>,
    ) -> Result<()> {
        // TODO: We temporarily assume that each opened value fits in both fields.
        witness.set_targets(
            &self.o_constants,
            &InnerSF::try_convert_all::<InnerBF>(&values.o_constants)?,
        );
        witness.set_targets(
            &self.o_plonk_sigmas,
            &InnerSF::try_convert_all::<InnerBF>(&values.o_plonk_sigmas)?,
        );
        witness.set_targets(
            &self.o_wires,
            &InnerSF::try_convert_all::<InnerBF>(&values.o_wires)?,
        );
        witness.set_target(self.o_plonk_z, values.o_plonk_z.try_convert::<InnerBF>()?);
        witness.set_targets(
            &self.o_plonk_t,
            &InnerSF::try_convert_all::<InnerBF>(&values.o_plonk_t)?,
        );
        witness.set_targets(
            &self.o_old_proofs,
            &InnerSF::try_convert_all::<InnerBF>(&values.o_old_proofs)?,
        );
        Ok(())
    }
}
