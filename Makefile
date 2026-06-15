.PHONY: build build-front dev deb install check fmt clean

build:        ## Compile le binaire du module
	cargo build --release --bin kubuno-mail

build-front:  ## Build le bundle frontend (dist/entry.js)
	cd frontend && npm run build

dev:          ## Lance le module en watch
	cargo watch -q -c -x 'run --bin kubuno-mail'

deb:          ## Construit le paquet Debian
	bash build_deb.sh

install:      ## Construit et installe le paquet
	bash build_deb.sh --install

check:        ## cargo check + typecheck frontend
	cargo check --bin kubuno-mail
	cd frontend && npm run typecheck

fmt:          ## Formate le code
	cargo fmt

clean:        ## Nettoie les artefacts
	cargo clean
	rm -rf frontend/dist dist
