What is rascaline
=================

Rascaline is a library for the efficient computing of representations for
atomistic machine learning also called "descriptors" or "fingerprints". These
representation can be used for atomistic machine learning (ML) models including
ML potentials, visualization or similarity analysis.

There exist several libraries able to compute such structural representations,
such as `DScribe`_, `QUIP`_, and many more. Rascaline tries to distinguish
itself by focussing on speed and memory efficiency of the calculations, with the
explicit goal of running molecular simulations with ML potentials. In
particular, memory efficiency is achieved by using the `equistore`_ to store the
structural representation. Additionally, rascaline is not limited to a single
representation but supports several:

.. include:: ../../../README.rst
   :start-after: inclusion-marker-representations-start
   :end-before: inclusion-marker-representations-end


.. _DScribe: https://singroup.github.io/dscribe/
.. _QUIP: https://www.libatoms.org
.. _equistore: https://lab-cosmo.github.io/equistore/
