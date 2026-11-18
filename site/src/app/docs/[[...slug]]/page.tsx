import type { Metadata } from "next";
import { notFound } from "next/navigation";

import * as Typography from "@/components/ui/typography";
import { source } from "@/lib/source";

type Props = { params: Promise<{ slug?: string[] }> };

export default async function Page({ params }: Props) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  const Body = page.data.body;

  return (
    <main>
      <Typography.H1>{page.data.title}</Typography.H1>
      <Typography.Lead>{page.data.description}</Typography.Lead>
      <Body
        components={{
          h2: Typography.H2,
          h3: Typography.H3,
          h4: Typography.H4,
          p: Typography.P,
          ul: Typography.List,
          blockquote: Typography.Blockquote,
        }}
      />
    </main>
  );
}

export function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  return { title: page.data.title, description: page.data.description };
}
