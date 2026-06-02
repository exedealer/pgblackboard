.PHONY: ui/_vendor/* server/_vendor/*

MAKEFLAGS += --silent
.SHELLFLAGS := -xc

export COMPOSE_BAKE=true

.PHONY: up
up:
	docker compose up --build --watch --menu=false dev pg18

.PHONY: up10
up10:
	docker compose up --build --watch --menu=false dev pg10

.PHONY: produp
produp:
	docker compose up --build --menu=false prod pg18

.PHONY: devcon
devcon:
	docker compose run --build --rm --volume $(PWD):/w --workdir /w dev ash

.PHONY: clean
clean:
	cargo clean -q
	rm -rf ui/.bundle

.PHONY: build
build: ui-bundle
	cargo build --release --frozen

.PHONY: ui-bundle
ui-bundle:
	rm -rf ui/.bundle

	esbuild ui/index.html ui/main.js ui/style.css \
	    --outdir=ui/.bundle \
		--bundle \
		--format=esm \
		--splitting \
		--chunk-names=[name] \
		--target=chrome100 \
		--loader:.svg=dataurl \
		--loader:.woff2=dataurl \
		--loader:.html=copy

	# TODO move favicon to esbuild, requires --loader:.svg=copy .
	# Currently --loader:.svg=dataurl conflicts with favicon.svg.
	# possible solution is to use --loader:.icon.svg=dataurl .
	# This is also solve potential issue with big svg uncontrollable inlining.
	cp ui/favicon.svg ui/.bundle/favicon.svg
	# brotli -- ui/.bundle/*.js ui/.bundle/*.css
	gzip -9 -k ui/.bundle/*.js ui/.bundle/*.css
	du -ahs ui/.bundle/* | sort -rh

ui/_vendor/vue.js:
	# TODO https://unpkg.com/vue@3.5.13/dist/vue.esm-browser.prod.js
	wget -O $@ 'https://unpkg.com/vue@3.5.13/dist/vue.esm-browser.js'

ui/_vendor/maplibre.css:
	wget -O $@ 'https://esm.sh/maplibre-gl@5.16.0/dist/maplibre-gl.css'
	deno fmt $@
ui/_vendor/maplibre.js:
	wget -O $@ 'https://esm.sh/v135/maplibre-gl@5.16.0/es2022/dist/maplibre-gl-dev.development.bundle.js'
	sed -i '/^\/\/# sourceMappingURL/d' $@

ui/_vendor/monaco.css:
	wget -O $@ 'https://esm.sh/monaco-editor@0.55.1/es2022/monaco-editor.css'
	deno fmt $@
ui/_vendor/monaco.js:
	wget -O $@ 'https://esm.sh/v135/monaco-editor@0.55.1/es2022/esm/vs/editor/editor.main.development.bundle.js'
ui/_vendor/monaco_worker.js:
	wget -O $@ 'https://esm.sh/v135/monaco-editor@0.55.1/es2022/esm/vs/editor/editor.worker.development.bundle.js?worker'
ui/_vendor/monaco_json_worker.js:
	wget -O $@ 'https://esm.sh/v135/monaco-editor@0.55.1/es2022/esm/vs/language/json/json.worker.development.bundle.js?worker'
