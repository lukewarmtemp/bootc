prefix ?= /usr

all:
	cargo build --release
    
install:
	install -D -m 0755 -t $(DESTDIR)$(prefix)/bin target/release/bootc
	install -d -m 0755 $(DESTDIR)$(prefix)/lib/systemd/system-generators/
	ln -f $(DESTDIR)$(prefix)/bin/bootc $(DESTDIR)$(prefix)/lib/systemd/system-generators/bootc-systemd-generator
	install -d $(DESTDIR)$(prefix)/lib/bootc/install
	# Support installing pre-generated man pages shipped in source tarball, to avoid
	# a dependency on pandoc downstream.  But in local builds these end up in target/man,
	# so we honor that too.
	for d in man target/man; do \
	  if test -d $$d; then \
	    install -D -m 0644 -t $(DESTDIR)$(prefix)/share/man/man5 $$d/*.5; \
	    install -D -m 0644 -t $(DESTDIR)$(prefix)/share/man/man8 $$d/*.8; \
	  fi; \
	  done
	install -D -m 0644 -t $(DESTDIR)/$(prefix)/lib/systemd/system systemd/*.service systemd/*.timer

install-with-tests: install
	install -D -m 0755 target/release/tests-integration $(DESTDIR)$(prefix)/bin/bootc-integration-tests 

bin-archive: all
	$(MAKE) install DESTDIR=tmp-install && tar --zstd -C tmp-install -cf target/bootc.tar.zst . && rm tmp-install -rf

test-bin-archive: all
	$(MAKE) install-with-tests DESTDIR=tmp-install && tar --zstd -C tmp-install -cf target/bootc.tar.zst . && rm tmp-install -rf

install-kola-tests:
	install -D -t $(DESTDIR)$(prefix)/lib/coreos-assembler/tests/kola/bootc tests/kolainst/*

validate:
	cargo fmt
	cargo clippy
.PHONY: validate

vendor:
	cargo xtask $@
.PHONY: vendor

package-rpm:
	cargo xtask $@
.PHONY: package-rpm
