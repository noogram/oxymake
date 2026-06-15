;;; manifest.scm — baseline hermetic environment for the OX-8 R0
;;; orthogonality attestation bench.
;;;
;;; Use:
;;;   guix shell -m manifest.scm -- <command>
;;;
;;; The bench needs `cat` (coreutils) and `bash` to execute the
;;; workflow's shell body. `ox` itself is NOT in the manifest — we
;;; deliberately use the host-installed OxyMake binary, because R0
;;; is testing the orthogonality between OxyMake's content-addressable
;;; cache (OX-1) and the Guix store hash, not a Guix-packaged ox.
;;;
;;; run.sh derives a `manifest-drift.scm` from this file at runtime
;;; to drive the store-hash drift axis (swaps `coreutils` for
;;; `coreutils-minimal`, which has a distinct /gnu/store/<hash>
;;; for `cat`).
;;;
;;; Provenance: orthogonality benchmark r0.
(specifications->manifest
 '("coreutils"
   "bash"))
