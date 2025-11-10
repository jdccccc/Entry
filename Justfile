release:
    cargo install --path . --root ./release

run:
    ./release/bin/Entry

clean:
    cargo clean
    cargo cache -a