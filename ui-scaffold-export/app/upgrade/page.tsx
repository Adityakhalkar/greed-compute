import { Nav } from '@/components/nav'
import { Pricing } from '@/components/pricing'
import { Footer } from '@/components/footer'

export default function UpgradePage() {
  return (
    <main>
      <Nav />
      <div className="pt-12">
        <Pricing />
      </div>
      <Footer />
    </main>
  )
}
