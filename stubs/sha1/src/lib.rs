// We have to keep the sha1 crate in the graph, but for Windows builds
// it will never be used (since we enable Windows CNG). This ensures
// that we never pull in the upstream sha1 crate.
