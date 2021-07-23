# fuse-rm

## About

`fuse-rm` allows you to access the reMarkable library (in
`/home/root/.local/share/remarkable/xochitl`) or its backup as normal filesystem.

This is useful on device for having a single library between `xochitl` and
`KOReader` (or any other reader).

To mount a xochitl directory (filled with *.metadata, *.pdf, etc.):

    fuse-rm xochitl-dir mountpoint

## Development Status

### Implemented:

* folder structure
* epubs and pdfs


### TODO:

* adding epubs and pdfs, creating folders
* moving/removing files and folders
* access to annotations and notes (rendering lines-files needed)

## Building and Installation

For a local build, just use `cargo`.

To build for the reMarkable (uses podman to run the toolchain):

    make toltec
    
Assuming that the device is accessible at `root@remarkable` with key-based
auth:
    
    make deploy-rm

## Notes

Before testing on a live device, it is a good idea to backup your xochitl
directory.
