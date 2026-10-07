.PHONY: build build-front dev kbpkg install check fmt clean

# Repository layout (README "Architecture"): server/ the Rust server, web/ the web frontend,
# mobile/ the mobile apps.

build:        ## Build the module's server binary
	cd server && cargo build --release --bin kubuno-mail

build-front:  ## Build the frontend bundle (dist/entry.js)
	cd web && npm run build

dev:          ## Run the server in watch mode
	cd server && cargo watch -q -c -x 'run --bin kubuno-mail'

kbpkg:        ## Build the Kubuno package (.kbpkg)
	bash build_kbpkg.sh

install:      ## Build and install the package into the local core
	bash build_kbpkg.sh --install

check:        ## cargo check + frontend typecheck
	cd server && cargo check --bin kubuno-mail
	cd web && npm run typecheck

fmt:          ## Format the code
	cd server && cargo fmt

clean:        ## Remove build outputs
	cd server && cargo clean
	rm -rf web/dist dist
