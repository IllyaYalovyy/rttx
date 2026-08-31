# Lets COPR build the SRPM itself from this git repository, so a build can be
# started from the COPR web UI ("Rebuild") or a GitHub webhook without any
# local tooling. COPR runs `make -f .copr/Makefile srpm outdir=... spec=...`
# as root in a network-enabled Fedora chroot. See packaging/rttx/rpm/README.md.
.PHONY: srpm
srpm:
	dnf -y install --setopt=install_weak_deps=False git cargo rpm-build xz
	git config --global --add safe.directory '*'
	./packaging/rttx/rpm/build-srpm.sh --outdir "$(outdir)"
