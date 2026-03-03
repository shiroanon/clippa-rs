# Maintainer: shiro shiroanony@gmail.com
pkgname=clippa-rs
pkgver=0.1.1
pkgrel=1
pkgdesc="A clipboard manager tui/service for wayland based compositors written in rust"
arch=(x86_64 i686)
url="https://github.com/shiroanon/clippa-rs"
license=('MIT')
depends=(gcc-libs libgcc_s.so
  glibc wl-clipboard)
makedepends=(cargo git)
source=("$pkgname-$pkgver.tar.gz::https://github.com/shiroanon/clippa-rs/archive/v$pkgver.tar.gz"
  "clippa-harvest.service::https://raw.githubusercontent.com/shiroanon/clippa-rs/v$pkgver/clippa-harvest.service")
sha256sums=('SKIP' 'SKIP')

build() {
  cd "$pkgname-$pkgver"
  cargo build --release --locked
}

package() {
  cd "$pkgname-$pkgver"

  install -Dm755 "target/release/clippa-harvest" "$pkgdir/usr/bin/${pkgname}"
  install -Dm755 "target/release/clippa-manage" "$pkgdir/usr/bin/${pkgname}"
  install -Dm644 "$srcdir/clippa-harvest.service" "$pkgdir/usr/lib/systemd/user/clippa-harvest.service"

}
