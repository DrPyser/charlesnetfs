use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnixPath(Vec<String>);

impl UnixPath {
    pub const ROOT: Self = Self(vec![]);

    pub fn new(s: &str) -> Self {
        if s.is_empty() {
            return Self::ROOT;
        }
        let components: Vec<String> = s
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        Self(components)
    }

    pub fn root() -> Self {
        Self::ROOT
    }

    pub fn is_absolute(&self) -> bool {
        true
    }

    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            return None;
        }
        let mut parent = self.0.clone();
        parent.pop();
        Some(Self(parent))
    }

    pub fn file_name(&self) -> Option<&str> {
        self.0.last().map(|s| s.as_str())
    }

    pub fn push(&mut self, component: &str) {
        if component.is_empty() {
            return;
        }
        self.0.push(component.to_string());
    }

    pub fn join(&self, component: &str) -> Self {
        let mut new = self.clone();
        new.push(component);
        new
    }

    pub fn ends_with(&self, component: &str) -> bool {
        self.0.last().is_some_and(|s| s == component)
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|s| s.as_str())
    }

    pub fn into_vec(self) -> Vec<String> {
        self.0
    }

    pub fn as_str_vec(&self) -> Vec<&str> {
        self.0.iter().map(|s| s.as_str()).collect()
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&str> for UnixPath {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<&UnixPath> for UnixPath {
    fn from(p: &UnixPath) -> Self {
        p.clone()
    }
}

impl From<String> for UnixPath {
    fn from(s: String) -> Self {
        Self::new(&s)
    }
}

impl From<UnixPath> for String {
    fn from(p: UnixPath) -> String {
        p.to_string()
    }
}

impl From<UnixPath> for Vec<String> {
    fn from(p: UnixPath) -> Vec<String> {
        p.0
    }
}

impl<'a> From<&'a UnixPath> for Vec<&'a str> {
    fn from(p: &'a UnixPath) -> Vec<&'a str> {
        p.as_str_vec()
    }
}

impl fmt::Display for UnixPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            write!(f, "/")
        } else {
            write!(f, "/{}", self.0.join("/"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root() {
        let p = UnixPath::new("/");
        assert!(p.is_root());
        assert_eq!(p.to_string(), "/");
    }

    #[test]
    fn test_single_component() {
        let p = UnixPath::new("/router");
        assert_eq!(p.components().collect::<Vec<_>>(), vec!["router"]);
        assert_eq!(p.file_name(), Some("router"));
    }

    #[test]
    fn test_multiple_components() {
        let p = UnixPath::new("/router/ping");
        assert_eq!(p.components().collect::<Vec<_>>(), vec!["router", "ping"]);
    }

    #[test]
    fn test_parent() {
        let p = UnixPath::new("/router/ping");
        let parent = p.parent();
        assert!(parent.is_some());
        assert_eq!(parent.unwrap().to_string(), "/router");
    }

    #[test]
    fn test_parent_of_root() {
        let p = UnixPath::ROOT;
        assert!(p.parent().is_none());
    }

    #[test]
    fn test_push() {
        let mut p = UnixPath::new("/router");
        p.push("ping");
        assert_eq!(p.to_string(), "/router/ping");
    }

    #[test]
    fn test_ends_with() {
        let p = UnixPath::new("/router/ping");
        assert!(p.ends_with("ping"));
        assert!(!p.ends_with("config"));
    }

    #[test]
    fn test_is_absolute() {
        let p = UnixPath::new("/router");
        assert!(p.is_absolute());
    }

    #[test]
    fn test_empty_string_is_root() {
        let p = UnixPath::new("");
        assert!(p.is_root());
        assert_eq!(p.to_string(), "/");
    }

    #[test]
    fn test_from_string() {
        let p: UnixPath = "/router/ping".into();
        assert_eq!(p.to_string(), "/router/ping");
    }

    #[test]
    fn test_into_string() {
        let p = UnixPath::new("/router/ping");
        let s: String = p.into();
        assert_eq!(s, "/router/ping");
    }

    #[test]
    fn test_ignores_trailing_slash() {
        let p = UnixPath::new("/router/");
        assert_eq!(p.file_name(), Some("router"));
    }

    #[test]
    fn test_ignores_empty_components() {
        let p = UnixPath::new("///router//ping///");
        assert_eq!(p.to_string(), "/router/ping");
    }

    #[test]
    fn test_into_vec_string() {
        let p = UnixPath::new("/router/ping");
        let v: Vec<String> = p.into();
        assert_eq!(v, vec!["router", "ping"]);
    }

    #[test]
    fn test_into_vec_str() {
        let p = UnixPath::new("/router/ping");
        let v: Vec<&str> = (&p).into();
        assert_eq!(v, vec!["router", "ping"]);
    }

    #[test]
    fn test_join() {
        let p = UnixPath::new("/router");
        let joined = p.join("ping");
        assert_eq!(joined.to_string(), "/router/ping");
    }

    #[test]
    fn test_join_root() {
        let p = UnixPath::ROOT;
        let joined = p.join("router");
        assert_eq!(joined.to_string(), "/router");
    }

    #[test]
    fn test_join_nested() {
        let p = UnixPath::new("/a");
        let joined = p.join("b").join("c");
        assert_eq!(joined.to_string(), "/a/b/c");
    }
}
